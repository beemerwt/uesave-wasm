mod utils;
mod uesave;

use std::io::Cursor;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsValue;
use uesave::*;
use js_sys::{Array, ArrayBuffer, Function, JsString, Map, Object, Uint8Array};

// This defines the Node.js Buffer type
#[wasm_bindgen]
extern "C" {
    type Buffer;

    #[wasm_bindgen(method, getter)]
    fn buffer(this: &Buffer) -> ArrayBuffer;

    #[wasm_bindgen(method, getter, js_name = byteOffset)]
    fn byte_offset(this: &Buffer) -> u32;

    #[wasm_bindgen(method, getter)]
    fn length(this: &Buffer) -> u32;
}

// When the wee_alloc feature is enabled, use wee_alloc as the global
// allocator.
#[cfg(feature = "wee_alloc")]
#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;

#[wasm_bindgen]
extern {
    #[wasm_bindgen(js_namespace = console)]
    fn error(s: &str);

    #[wasm_bindgen(js_namespace = console)]
    fn log(s: &str);

}

fn map_to_object(map: &Map) -> JsValue {
    match Object::from_entries(map) {
        Ok(obj) => obj.into(),
        Err(e) => e
    }
}

impl From<Vector> for JsValue {
    fn from(val: Vector) -> Self {
        let array = Array::new_with_length(3);
        array.set(0, JsValue::from(val.x));
        array.set(1, JsValue::from(val.y));
        array.set(2, JsValue::from(val.z));
        array.into()
    }
}

impl From<Vector2D> for JsValue {
    fn from(val: Vector2D) -> Self {
        let array = Array::new_with_length(2);
        array.set(0, JsValue::from(val.x));
        array.set(1, JsValue::from(val.y));
        array.into()
    }
}

impl From<IntPoint> for JsValue {
    fn from(val: IntPoint) -> Self {
        let array = Array::new_with_length(2);
        array.set(0, JsValue::from(val.x));
        array.set(1, JsValue::from(val.y));
        array.into()
    }
}

impl From<Quat> for JsValue {
    fn from(val: Quat) -> Self {
        let array = Array::new_with_length(4);
        array.set(0, JsValue::from(val.x));
        array.set(1, JsValue::from(val.y));
        array.set(2, JsValue::from(val.z));
        array.set(3, JsValue::from(val.w));
        array.into()
    }
}

impl From<LinearColor> for JsValue {
    fn from(val: LinearColor) -> Self {
        let array = Array::new_with_length(4);
        array.set(0, JsValue::from(val.r));
        array.set(1, JsValue::from(val.g));
        array.set(2, JsValue::from(val.b));
        array.set(3, JsValue::from(val.a));
        array.into()
    }
}

impl From<Color> for JsValue {
    fn from(val: Color) -> Self {
        let array = Array::new_with_length(4);
        array.set(0, JsValue::from(val.r));
        array.set(1, JsValue::from(val.g));
        array.set(2, JsValue::from(val.b));
        array.set(3, JsValue::from(val.a));
        array.into()
    }
}

impl From<Rotator> for JsValue {
    fn from(val: Rotator) -> Self {
        let array = Array::new_with_length(3);
        array.set(0, JsValue::from(val.x));
        array.set(1, JsValue::from(val.y));
        array.set(2, JsValue::from(val.z));
        array.into()
    }
}

impl From<uesave::Box> for JsValue {
    fn from(value: uesave::Box) -> Self {
        let array = Array::new_with_length(2);
        array.set(0, JsValue::from(value.a));
        array.set(1, JsValue::from(value.b));
        array.into()
    }
}

impl From<Byte> for JsValue {
    fn from(value: Byte) -> Self {
        match value {
            Byte::Label(s) => JsValue::from(s),
            Byte::Byte(b) => JsValue::from(b)
        }
    }
}

impl From<StructValue> for JsValue {
    fn from(val: StructValue) -> Self {
        match val {
            StructValue::Guid(guid) => JsValue::from(guid.as_hyphenated().to_string()),
            StructValue::Box(b) => JsValue::from(b),
            StructValue::DateTime(dt) => JsValue::from(dt),
            StructValue::Timespan(i) => JsValue::from(i),
            StructValue::Vector2D(v) => JsValue::from(v),
            StructValue::Vector(v) => JsValue::from(v),
            StructValue::IntPoint(p) => JsValue::from(p),
            StructValue::Quat(q) => JsValue::from(q),
            StructValue::LinearColor(c) => JsValue::from(c),
            StructValue::Color(c) => JsValue::from(c),
            StructValue::Rotator(r) => JsValue::from(r),
            StructValue::SoftObjectPath(value, value2) => {
                let array = Array::new_with_length(2);
                array.set(0, JsValue::from(value));
                array.set(1, JsValue::from(value2));
                array.into()
            },
            StructValue::GameplayTagContainer(container) => {
                let array = Array::new();
                for tag in container.gameplay_tags {
                    array.push(&JsValue::from(tag.name));
                }
                array.into()
            },
            StructValue::Struct(properties) => {
                let map = Map::new();
                for (key, value) in properties {
                    map.set(&JsValue::from(key), &JsValue::from(value));
                }
                map_to_object(&map)
            }
            _ => JsValue::null()
        }
    }
}

impl From<ValueVec> for JsValue {
    fn from(value: ValueVec) -> Self {
        let array = Array::new();

        match value {
            ValueVec::Bool(v) => v.iter().for_each(|b| { array.push(&JsValue::from(*b)); }),
            ValueVec::Box(v) => v.iter().for_each(|b| { array.push(&JsValue::from(b.clone())); }),
            ValueVec::Double(v) => v.iter().for_each(|e| { array.push(&JsValue::from(*e)); }),
            ValueVec::Int8(v) => v.iter().for_each(|e| { array.push(&JsValue::from(*e)); }),
            ValueVec::Int16(v) => v.iter().for_each(|e| { array.push(&JsValue::from(*e)); }),
            ValueVec::Int(v) => v.iter().for_each(|e| { array.push(&JsValue::from(*e)); }),
            ValueVec::Int64(v) => v.iter().for_each(|e| { array.push(&JsValue::from(*e)); }),
            ValueVec::UInt8(v) => v.iter().for_each(|e| { array.push(&JsValue::from(*e)); }),
            ValueVec::UInt16(v) => v.iter().for_each(|e| { array.push(&JsValue::from(*e)); }),
            ValueVec::UInt32(v) => v.iter().for_each(|e| { array.push(&JsValue::from(*e)); }),
            ValueVec::UInt64(v) => v.iter().for_each(|e| { array.push(&JsValue::from(*e)); }),
            ValueVec::Float(v) => v.iter().for_each(|e| { array.push(&JsValue::from(*e)); }),
            ValueVec::Enum(v) => v.iter().for_each(|e| { array.push(&JsValue::from(e.clone())); }),
            ValueVec::Str(v) => v.iter().for_each(|e| { array.push(&JsValue::from(e.clone())); }),
            ValueVec::Name(v) => v.iter().for_each(|e| { array.push(&JsValue::from(e.clone())); }),
            ValueVec::Object(v) => v.iter().for_each(|e| { array.push(&JsValue::from(e.clone())); }),
            
            ValueVec::SoftObject(v) => {
                v.iter().for_each(|(a, b)| {
                    let sry = Array::new_with_length(2);
                    sry.set(0, JsValue::from(a));
                    sry.set(1, JsValue::from(b));
                    array.push(&sry.into());
                });
            },

            ValueVec::Byte(v) => {
                match v {
                    ByteArray::Byte(bry) => bry.iter().for_each(|b| { array.push(&JsValue::from(*b)); }),
                    ByteArray::Label(sry) => {
                        sry.iter().for_each(|s| { array.push(&JsValue::from(s)); });
                    }
                }
            },
            _ => {}
        };

        array.into()
    }
}

impl From<ValueSet> for JsValue {
    fn from(value: ValueSet) -> Self {
        match value {
            ValueSet::Base(v) => JsValue::from(v),
            ValueSet::Struct(v) => {
                let array = Array::new();
                for _struct in v {
                    array.push(&JsValue::from(_struct));
                }
                array.into()
            }
        }
    }
}

impl From<ValueArray> for JsValue {
    fn from(value: ValueArray) -> Self {
        match value {
            ValueArray::Base(b) => JsValue::from(b),
            ValueArray::Struct { _type, name, value, ..} => {
                let map = Map::new();
                let array = Array::new();
                for sval in value {
                    array.push(&JsValue::from(sval));
                }

                map.set(&JsString::from("Name").into(), &JsValue::from(name));
                map.set(&JsString::from("Type").into(), &JsValue::from(_type));
                map.set(&JsString::from("Values").into(), &array.into());
                map_to_object(&map)
            }
        }
    }
}

impl From<PropertyValue> for JsValue {
    fn from(value: PropertyValue) -> Self {
        match value {
            PropertyValue::Bool(b) => JsValue::from(b),
            PropertyValue::Int(v) => JsValue::from(v),
            PropertyValue::Int8(v) => JsValue::from(v),
            PropertyValue::Int16(v) => JsValue::from(v),
            PropertyValue::Int64(v) => JsValue::from(v),
            PropertyValue::UInt16(v) => JsValue::from(v),
            PropertyValue::UInt32(v) => JsValue::from(v),
            PropertyValue::Float(v) => JsValue::from(v),
            PropertyValue::Double(v) => JsValue::from(v),
            PropertyValue::Byte(v) => JsValue::from(v),
            PropertyValue::Enum(v) => JsValue::from(v),
            PropertyValue::Name(v) => JsValue::from(v),
            PropertyValue::Str(v) => JsValue::from(v),
            PropertyValue::Object(v) => JsValue::from(v),
            PropertyValue::Struct(v) => JsValue::from(v),
            PropertyValue::SoftObject(value, value2) |
            PropertyValue::SoftObjectPath(value, value2) => {
                let array = Array::new_with_length(2);
                array.set(0, JsValue::from(value));
                array.set(1, JsValue::from(value2));
                array.into()
            }
        }
    }
}

impl From<Property> for JsValue {
    fn from(prop: Property) -> Self {
        match prop {
            Property::Int8 { value, .. } => JsValue::from(value),
            Property::Int16 { value, .. } => JsValue::from(value),
            Property::Int { value, .. } => JsValue::from(value),
            Property::Int64 { value, .. } => JsValue::from(value),
            Property::UInt8 { value, .. } => JsValue::from(value),
            Property::UInt16 { value, .. } => JsValue::from(value),
            Property::UInt32 { value, .. } => JsValue::from(value),
            Property::UInt64 { value, .. } => JsValue::from(value),
            Property::Float { value, .. } => JsValue::from(value),
            Property::Double { value, .. } => JsValue::from(value),
            Property::Bool { value, .. } => JsValue::from(value),
            Property::Enum { value, .. } => JsValue::from(value),
            Property::Name { value, .. } => JsValue::from(value),
            Property::Str { value, .. } => JsValue::from(value),
            Property::Object { value, .. } => JsValue::from(value),
            Property::Byte { value, .. } => JsValue::from(value),

            Property::SoftObject { value, value2, .. } => {
                // String, String
                let ary = Array::new_with_length(2);
                ary.set(0, JsValue::from(value));
                ary.set(1, JsValue::from(value2));
                return ary.into();
            },

            Property::Text { value, .. } => {
                // uesave::Text
                match value.variant {
                    TextVariant::None { .. } => JsValue::null(),
                    TextVariant::Base { source_string, .. } => JsString::from(source_string).into(),
                    TextVariant::AsDate { source_date_time, .. } => JsValue::from(source_date_time),
                    TextVariant::ArgumentFormat { .. } => JsValue::null(),
                    TextVariant::AsNumber { source_value, .. } => {
                        match source_value {
                            FFormatArgumentValue::Int(i) => JsValue::from(i),
                            FFormatArgumentValue::UInt(ui) => JsValue::from(ui),
                            FFormatArgumentValue::Float(f) => JsValue::from(f),
                            FFormatArgumentValue::Double(d) => JsValue::from(d),
                            FFormatArgumentValue::Text(_) => JsValue::null(),
                            FFormatArgumentValue::Gender(g) => JsValue::from(g),
                        }
                    },
                    TextVariant::StringTableEntry { table, key } => {
                        JsString::from(key)
                            .concat(&JsString::from(table).into())
                            .into()
                    }
                }
            },

            Property::Delegate { value, .. } => {
                // uesave::Delegate
                // name: String, path: String
                JsString::from(value.path)
                    .concat(&JsString::from("/").into())
                    .concat(&JsString::from(value.name).into())
                    .into()
            },

            Property::Set { value, .. } => JsValue::from(value),
            Property::Struct { value, .. } => JsValue::from(value),
            Property::Array { value, .. } => JsValue::from(value),

            Property::Map { key_type, value_type, value, .. } => {
                let map = Map::new();
                map.set(&JsString::from("KeyType").into(), &JsString::from(key_type.get_name()).into());
                map.set(&JsString::from("ValueType").into(), &JsString::from(value_type.get_name()).into());

                let array = Array::new();
                for entry in value {
                    let map = Map::new();
                    map.set(&JsString::from("Key").into(), &JsValue::from(entry.key));
                    map.set(&JsString::from("Value").into(), &JsValue::from(entry.value));
                    array.push(&map_to_object(&map));
                }
                
                map.set(&JsString::from("Values").into(), &array.into());
                map_to_object(&map)
            },

            Property::RawData { properties, .. } => {
                let map = Map::new();
                for (key, value) in properties {
                    map.set(&JsValue::from(key), &JsValue::from(value));
                }
                map_to_object(&map)
            },

            _ => JsValue::null()
        }
    }
}

#[wasm_bindgen]
pub fn deserialize(buffer: &Uint8Array, map: Map, callback: &Function) -> Result<Object, JsValue> {
    utils::set_panic_hook();
    let mut types = Types::new();

    map.for_each(&mut |value, key| {
        types.add(key.as_string().unwrap(), StructType::Struct(value.as_string()));
    });

    let buf: Vec<u8> = buffer.to_vec();
    let mut file = Cursor::new(&buf);

    let save = Save::read_with_types(&mut file, &types, Some(callback));

    if save.is_err() {
        error("Read save failed");
        let s = save.err().unwrap().to_string();
        error(&s);
        panic!("{}", s);
    }
    
    let obj = Map::new();
    for (key, value) in save.unwrap().root.properties {
        obj.set(&JsValue::from(key), &JsValue::from(value));
    }

    Object::from_entries(&obj)
}

#[wasm_bindgen]
pub fn serialize(json: String) -> Vec<u8> {
    let decoded: Save = serde_json::from_str(&json).unwrap();
    let mut writer: Vec<u8> = Vec::new();
    let _result = decoded.write(&mut writer).unwrap();
    return writer;
}