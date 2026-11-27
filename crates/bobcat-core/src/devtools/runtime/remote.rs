//! CDP `RemoteObject`s, previews and property descriptors built from a
//! realm's values with the bridge's reflection entries, which run no script,
//! and the table of values held for the frontend by `objectId`.

use quickjs_rust_bridge::{
    self as quickjs, Context, DescribeLimits, ObjectClass, OwnKeys, OwnProperty, PromiseState,
    PropertyKey, PropertySlot, Reflected, Value, ValueKind,
};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;

use crate::devtools::protocol::Error;

const DESCRIBE: DescribeLimits = DescribeLimits {
    depth: 1,
    string_length: 100,
    item_count: 5,
};
const PREVIEW_PROPERTIES: usize = 5;
const PREVIEW_INDICES: u32 = 100;
const PREVIEW_STRING: usize = 100;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteObject {
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) subtype: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) class_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) unserializable_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) object_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) preview: Option<ObjectPreview>,
}

impl RemoteObject {
    pub(super) fn string(text: &str) -> Self {
        Self {
            kind: "string",
            value: Some(text.into()),
            ..Self::default()
        }
    }

    pub(super) fn error(class_name: &str, description: String) -> Self {
        Self {
            kind: "object",
            subtype: Some("error"),
            class_name: Some(class_name.to_owned()),
            description: Some(description),
            ..Self::default()
        }
    }

    fn preview_text(&self) -> Option<String> {
        self.description
            .clone()
            .or_else(|| match self.value.as_ref()? {
                serde_json::Value::String(text) => {
                    Some(text.chars().take(PREVIEW_STRING).collect())
                }
                other => Some(other.to_string()),
            })
            .or_else(|| (self.kind == "undefined").then(|| "undefined".to_owned()))
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ObjectPreview {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    subtype: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    overflow: bool,
    properties: Vec<PropertyPreview>,
}

#[derive(Clone, Serialize)]
struct PropertyPreview {
    name: String,
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subtype: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PropertyDescriptor {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<RemoteObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    writable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    get: Option<RemoteObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    set: Option<RemoteObject>,
    configurable: bool,
    enumerable: bool,
    is_own: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<RemoteObject>,
}

#[derive(Serialize)]
pub(super) struct InternalProperty {
    name: &'static str,
    value: RemoteObject,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Properties {
    result: Vec<PropertyDescriptor>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    internal_properties: Vec<InternalProperty>,
}

pub(super) struct Held {
    pub(super) value: Value,
    pub(super) group: Option<String>,
}

#[derive(Default)]
pub(super) struct ObjectTable {
    last: u64,
    held: FxHashMap<u64, Held>,
}

impl ObjectTable {
    fn hold(&mut self, value: &Value, group: Option<&str>) -> String {
        self.last += 1;
        self.held.insert(
            self.last,
            Held {
                value: value.clone(),
                group: group.map(str::to_owned),
            },
        );
        self.last.to_string()
    }

    pub(super) fn get(&self, id: &str) -> Result<&Held, Error> {
        id.parse()
            .ok()
            .and_then(|id| self.held.get(&id))
            .ok_or_else(missing_object)
    }

    pub(super) fn release(&mut self, id: &str) -> Result<(), Error> {
        id.parse()
            .ok()
            .and_then(|id| self.held.remove(&id))
            .map(drop)
            .ok_or_else(missing_object)
    }

    pub(super) fn release_group(&mut self, group: &str) {
        self.held
            .retain(|_, held| held.group.as_deref() != Some(group));
    }

    pub(super) fn clear(&mut self) {
        self.held.clear();
    }
}

fn missing_object() -> Error {
    Error::server("Could not find object with given id")
}

pub(super) fn reflection(error: &quickjs::Error) -> Error {
    Error::server(format!("the value could not be read: {}", error.message))
}

pub(super) struct Mirror<'a> {
    pub(super) realm: &'a Context,
    pub(super) objects: &'a mut ObjectTable,
    pub(super) group: Option<&'a str>,
}

impl Mirror<'_> {
    pub(super) fn remote(
        &mut self,
        value: &Value,
        preview: bool,
    ) -> Result<RemoteObject, quickjs::Error> {
        let mut remote = shape(self.realm, value)?;
        if matches!(remote.kind, "symbol" | "function" | "object") && remote.subtype != Some("null")
        {
            remote.object_id = Some(self.objects.hold(value, self.group));
            if preview && remote.kind == "object" {
                remote.preview = Some(self.preview(value, &remote)?);
            }
        }
        Ok(remote)
    }

    fn preview(
        &self,
        value: &Value,
        remote: &RemoteObject,
    ) -> Result<ObjectPreview, quickjs::Error> {
        let indexed = match self.realm.object_class(value)? {
            Some(ObjectClass::Array) => Some(array_length(self.realm, value)?),
            Some(ObjectClass::TypedArray { length }) => {
                Some(u32::try_from(length).map_or(f64::MAX, f64::from))
            }
            _ => None,
        };
        let mut properties = Vec::new();
        let overflow = if let Some(length) = indexed {
            for index in (0..PREVIEW_INDICES).take_while(|index| f64::from(*index) < length) {
                let name = index.to_string();
                if let Reflected::Read(Some(property)) = self.realm.own_property(value, &name)? {
                    properties.push(self.property_preview(name, property.slot)?);
                }
            }
            length > f64::from(PREVIEW_INDICES)
        } else {
            let own = match self.realm.own_properties(value, OwnKeys::All)? {
                Reflected::Read(own) => own,
                Reflected::Proxy => Vec::new(),
            };
            let mut shown = own.into_iter().filter(|property| {
                property.enumerable && matches!(property.key, PropertyKey::String(_))
            });
            for property in shown.by_ref().take(PREVIEW_PROPERTIES) {
                if let PropertyKey::String(name) = property.key {
                    properties.push(self.property_preview(name, property.slot)?);
                }
            }
            shown.next().is_some()
        };
        Ok(ObjectPreview {
            kind: remote.kind,
            subtype: remote.subtype,
            description: remote.description.clone(),
            overflow,
            properties,
        })
    }

    fn property_preview(
        &self,
        name: String,
        slot: PropertySlot,
    ) -> Result<PropertyPreview, quickjs::Error> {
        Ok(match slot {
            PropertySlot::Accessor { .. } => PropertyPreview {
                name,
                kind: "accessor",
                value: None,
                subtype: None,
            },
            PropertySlot::Data { value, .. } => {
                let shown = shape(self.realm, &value)?;
                PropertyPreview {
                    name,
                    kind: shown.kind,
                    value: shown.preview_text(),
                    subtype: shown.subtype,
                }
            }
        })
    }

    pub(super) fn properties(
        &mut self,
        object: &Value,
        own_only: bool,
        accessors_only: bool,
        keys: OwnKeys,
        preview: bool,
    ) -> Result<Properties, quickjs::Error> {
        let mut properties = Properties {
            result: Vec::new(),
            internal_properties: Vec::new(),
        };
        if !matches!(object.kind(), ValueKind::Object | ValueKind::Function) {
            return Ok(properties);
        }
        let class = self.realm.object_class(object)?;
        if matches!(class, Some(ObjectClass::Proxy)) {
            return Ok(properties);
        }
        let mut seen = FxHashSet::default();
        let mut current = object.clone();
        let mut is_own = true;
        loop {
            let Reflected::Read(own) = self.realm.own_properties(&current, keys)? else {
                break;
            };
            for property in own {
                if accessors_only && !matches!(property.slot, PropertySlot::Accessor { .. }) {
                    continue;
                }
                if let PropertyKey::String(name) = &property.key
                    && !seen.insert(name.clone())
                {
                    continue;
                }
                properties
                    .result
                    .push(self.descriptor(property, is_own, preview)?);
            }
            if own_only {
                break;
            }
            match self.realm.prototype(&current)? {
                Reflected::Read(Some(prototype)) => current = prototype,
                _ => break,
            }
            is_own = false;
        }
        if accessors_only {
            return Ok(properties);
        }
        if let Reflected::Read(Some(prototype)) = self.realm.prototype(object)? {
            properties.internal_properties.push(InternalProperty {
                name: "[[Prototype]]",
                value: self.remote(&prototype, preview)?,
            });
        }
        if let Some(ObjectClass::Promise(state)) = class {
            let (state, result) = match state {
                PromiseState::Pending => ("pending", None),
                PromiseState::Fulfilled(value) => ("fulfilled", Some(value)),
                PromiseState::Rejected(value) => ("rejected", Some(value)),
            };
            properties.internal_properties.push(InternalProperty {
                name: "[[PromiseState]]",
                value: RemoteObject::string(state),
            });
            properties.internal_properties.push(InternalProperty {
                name: "[[PromiseResult]]",
                value: match result {
                    Some(value) => self.remote(&value, preview)?,
                    None => RemoteObject {
                        kind: "undefined",
                        ..RemoteObject::default()
                    },
                },
            });
        }
        Ok(properties)
    }

    fn descriptor(
        &mut self,
        property: OwnProperty,
        is_own: bool,
        preview: bool,
    ) -> Result<PropertyDescriptor, quickjs::Error> {
        let (name, symbol) = match property.key {
            PropertyKey::String(name) => (name, None),
            PropertyKey::Symbol { symbol, .. } => (
                self.realm.describe(&symbol, DESCRIBE)?,
                Some(self.remote(&symbol, false)?),
            ),
        };
        let mut descriptor = PropertyDescriptor {
            name,
            value: None,
            writable: None,
            get: None,
            set: None,
            configurable: property.configurable,
            enumerable: property.enumerable,
            is_own,
            symbol,
        };
        match property.slot {
            PropertySlot::Data { value, writable } => {
                descriptor.value = Some(self.remote(&value, preview)?);
                descriptor.writable = Some(writable);
            }
            PropertySlot::Accessor { getter, setter } => {
                descriptor.get = Some(self.accessor(getter.as_ref())?);
                descriptor.set = Some(self.accessor(setter.as_ref())?);
            }
        }
        Ok(descriptor)
    }

    fn accessor(&mut self, function: Option<&Value>) -> Result<RemoteObject, quickjs::Error> {
        match function {
            Some(function) => self.remote(function, false),
            None => Ok(RemoteObject {
                kind: "undefined",
                ..RemoteObject::default()
            }),
        }
    }
}

pub(super) fn by_value(realm: &Context, value: &Value) -> Result<RemoteObject, Error> {
    let kind = match value.kind() {
        ValueKind::Symbol => "symbol",
        ValueKind::Function => "function",
        ValueKind::Object => "object",
        _ => return shape(realm, value).map_err(|error| reflection(&error)),
    };
    let text = realm
        .json_stringify(value)
        .map_err(|_| Error::server("Object couldn't be returned by value"))?;
    Ok(match text {
        Some(text) => RemoteObject {
            kind,
            value: Some(
                serde_json::from_str(&text)
                    .map_err(|_| Error::server("Object couldn't be returned by value"))?,
            ),
            ..RemoteObject::default()
        },
        None => RemoteObject {
            kind: "undefined",
            ..RemoteObject::default()
        },
    })
}

fn shape(realm: &Context, value: &Value) -> Result<RemoteObject, quickjs::Error> {
    let primitive = |kind| RemoteObject {
        kind,
        ..RemoteObject::default()
    };
    Ok(match value.kind() {
        ValueKind::Null => RemoteObject {
            subtype: Some("null"),
            value: Some(serde_json::Value::Null),
            ..primitive("object")
        },
        ValueKind::Boolean => RemoteObject {
            value: value.as_boolean().map(Into::into),
            ..primitive("boolean")
        },
        ValueKind::String => RemoteObject {
            value: Some(string_of(value).unwrap_or_default().into()),
            ..primitive("string")
        },
        ValueKind::Number => number(realm, value)?,
        ValueKind::BigInt => {
            let text = format!("{}n", value.to_big_int_decimal()?);
            RemoteObject {
                unserializable_value: Some(text.clone()),
                description: Some(text),
                ..primitive("bigint")
            }
        }
        ValueKind::Symbol => RemoteObject {
            description: Some(realm.describe(value, DESCRIBE)?),
            ..primitive("symbol")
        },
        ValueKind::Function | ValueKind::Object => object(realm, value)?,
        _ => primitive("undefined"),
    })
}

fn number(realm: &Context, value: &Value) -> Result<RemoteObject, quickjs::Error> {
    let number = value.as_number().unwrap_or(f64::NAN);
    let special = if number.is_nan() {
        Some("NaN")
    } else if number.is_infinite() {
        Some(if number > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        })
    } else if number == 0.0 && number.is_sign_negative() {
        Some("-0")
    } else {
        None
    };
    Ok(match special {
        Some(text) => RemoteObject {
            kind: "number",
            unserializable_value: Some(text.to_owned()),
            description: Some(text.to_owned()),
            ..RemoteObject::default()
        },
        None => RemoteObject {
            kind: "number",
            value: Some(json_number(number)),
            description: Some(realm.describe(value, DESCRIBE)?),
            ..RemoteObject::default()
        },
    })
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "only an integral number within the exact range is converted"
)]
fn json_number(number: f64) -> serde_json::Value {
    const EXACT: f64 = 9_007_199_254_740_992.0;
    if number.fract() == 0.0 && number.abs() < EXACT {
        (number as i64).into()
    } else {
        number.into()
    }
}

fn object(realm: &Context, value: &Value) -> Result<RemoteObject, quickjs::Error> {
    let kind = if value.kind() == ValueKind::Function {
        "function"
    } else {
        "object"
    };
    let Some(class) = realm.object_class(value)? else {
        return Ok(RemoteObject {
            kind: "undefined",
            ..RemoteObject::default()
        });
    };
    let (subtype, own_name) = match &class {
        ObjectClass::Proxy => {
            return Ok(RemoteObject {
                kind,
                subtype: Some("proxy"),
                class_name: Some("Object".to_owned()),
                description: Some("Proxy".to_owned()),
                ..RemoteObject::default()
            });
        }
        ObjectClass::Function => {
            let name = data_string(realm, value, "name")?.unwrap_or_default();
            return Ok(RemoteObject {
                kind,
                class_name: Some("Function".to_owned()),
                description: Some(format!("function {name}()")),
                ..RemoteObject::default()
            });
        }
        ObjectClass::Array => (Some("array"), "Array"),
        ObjectClass::Error => (Some("error"), "Error"),
        ObjectClass::Promise(_) => (Some("promise"), "Promise"),
        ObjectClass::TypedArray { .. } => (Some("typedarray"), "Object"),
        ObjectClass::Map => (Some("map"), "Map"),
        ObjectClass::Set => (Some("set"), "Set"),
        ObjectClass::Date => (Some("date"), "Date"),
        ObjectClass::RegExp => (Some("regexp"), "RegExp"),
        ObjectClass::Object => (None, "Object"),
    };
    let class_name = constructor_name(realm, value)?.unwrap_or_else(|| own_name.to_owned());
    let description = match class {
        ObjectClass::Array => format!("Array({})", array_length(realm, value)?),
        ObjectClass::TypedArray { length } => format!("{class_name}({length})"),
        ObjectClass::Error
        | ObjectClass::Date
        | ObjectClass::RegExp
        | ObjectClass::Map
        | ObjectClass::Set => realm.describe(value, DESCRIBE)?,
        ObjectClass::Promise(_) => "Promise".to_owned(),
        _ => class_name.clone(),
    };
    Ok(RemoteObject {
        kind,
        subtype,
        class_name: Some(class_name),
        description: Some(description),
        ..RemoteObject::default()
    })
}

fn array_length(realm: &Context, array: &Value) -> Result<f64, quickjs::Error> {
    Ok(match realm.own_property(array, "length")? {
        Reflected::Read(Some(OwnProperty {
            slot: PropertySlot::Data { value, .. },
            ..
        })) => value.as_number().unwrap_or(0.0),
        _ => 0.0,
    })
}

fn constructor_name(realm: &Context, value: &Value) -> Result<Option<String>, quickjs::Error> {
    let mut current = value.clone();
    while let Reflected::Read(Some(prototype)) = realm.prototype(&current)? {
        match realm.own_property(&prototype, "constructor")? {
            Reflected::Proxy => return Ok(None),
            Reflected::Read(Some(OwnProperty {
                slot:
                    PropertySlot::Data {
                        value: constructor, ..
                    },
                ..
            })) if constructor.kind() == ValueKind::Function => {
                return Ok(
                    data_string(realm, &constructor, "name")?.filter(|name| !name.is_empty())
                );
            }
            Reflected::Read(_) => current = prototype,
        }
    }
    Ok(None)
}

fn data_string(
    realm: &Context,
    object: &Value,
    name: &str,
) -> Result<Option<String>, quickjs::Error> {
    Ok(match realm.own_property(object, name)? {
        Reflected::Read(Some(OwnProperty {
            slot: PropertySlot::Data { value, .. },
            ..
        })) => string_of(&value),
        _ => None,
    })
}

fn string_of(value: &Value) -> Option<String> {
    value
        .to_utf16()
        .ok()
        .map(|units| String::from_utf16_lossy(&units))
}
