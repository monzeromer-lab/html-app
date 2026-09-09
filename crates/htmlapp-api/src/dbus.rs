//! `dbus` — the session and system buses (PRD §9.3 Tier 2).
//!
//! §9.3 calls this "the single highest-leverage API on Linux", and it is: NetworkManager, UPower,
//! logind, BlueZ, and MPRIS all become reachable without the runtime wrapping any of them. That
//! leverage cuts both ways, which is why every call is checked against `dbus.destinations` first.

use htmlapp_bridge::RpcError;
use htmlapp_bridge::dispatch::{ApiHandler, BoxFuture, ValueStream};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::context::Ctx;
use crate::params::decode;

pub struct DbusModule {
    ctx: Ctx,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallParams {
    #[serde(default)]
    bus: Bus,
    destination: String,
    path: String,
    #[serde(rename = "iface")]
    interface: String,
    member: String,
    #[serde(default)]
    args: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PropertyParams {
    #[serde(default)]
    bus: Bus,
    destination: String,
    path: String,
    #[serde(rename = "iface")]
    interface: String,
    property: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IntrospectParams {
    #[serde(default)]
    bus: Bus,
    destination: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscribeParams {
    #[serde(default)]
    bus: Bus,
    #[serde(default)]
    destination: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default, rename = "iface")]
    interface: Option<String>,
    #[serde(default)]
    member: Option<String>,
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Bus {
    #[default]
    Session,
    System,
}

impl Bus {
    fn is_system(self) -> bool {
        self == Bus::System
    }
}

impl DbusModule {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx }
    }
}

#[cfg(feature = "tier2")]
impl DbusModule {
    async fn connect(&self, bus: Bus) -> Result<zbus::Connection, RpcError> {
        let result = if bus.is_system() {
            zbus::Connection::system().await
        } else {
            zbus::Connection::session().await
        };
        result.map_err(|e| RpcError::internal(format!("could not reach the bus: {e}")))
    }

    async fn call(&self, params: Value) -> Result<Value, RpcError> {
        let params: CallParams = decode("dbus.call", params)?;
        self.ctx.check_dbus(params.bus.is_system(), &params.destination)?;

        let connection = self.connect(params.bus).await?;
        // Arguments are passed as strings: expressing arbitrary D-Bus signatures from JSON needs a
        // signature the caller has not given us, and guessing would silently send the wrong type.
        let args: Vec<String> = params
            .args
            .iter()
            .map(|a| match a {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect();

        let reply = connection
            .call_method(
                Some(params.destination.as_str()),
                params.path.as_str(),
                Some(params.interface.as_str()),
                params.member.as_str(),
                &args,
            )
            .await
            .map_err(|e| RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;

        let body = reply.body();
        Ok(json!(format!("{:?}", body.signature())))
    }

    async fn introspect(&self, params: Value) -> Result<Value, RpcError> {
        let params: IntrospectParams = decode("dbus.introspect", params)?;
        self.ctx.check_dbus(params.bus.is_system(), &params.destination)?;

        let connection = self.connect(params.bus).await?;
        let reply = connection
            .call_method(
                Some(params.destination.as_str()),
                params.path.as_str(),
                Some("org.freedesktop.DBus.Introspectable"),
                "Introspect",
                &(),
            )
            .await
            .map_err(|e| RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;

        let xml: String = reply
            .body()
            .deserialize()
            .map_err(|e| RpcError::internal(e.to_string()))?;
        Ok(json!(xml))
    }

    async fn get_property(&self, params: Value) -> Result<Value, RpcError> {
        let params: PropertyParams = decode("dbus.get", params)?;
        self.ctx.check_dbus(params.bus.is_system(), &params.destination)?;

        let connection = self.connect(params.bus).await?;
        let reply = connection
            .call_method(
                Some(params.destination.as_str()),
                params.path.as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(params.interface.as_str(), params.property.as_str()),
            )
            .await
            .map_err(|e| RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;

        let value: zbus::zvariant::OwnedValue = reply
            .body()
            .deserialize()
            .map_err(|e| RpcError::internal(e.to_string()))?;
        Ok(variant_to_json(&value))
    }
}

/// Convert a D-Bus variant into JSON, as faithfully as JSON allows.
#[cfg(feature = "tier2")]
fn variant_to_json(value: &zbus::zvariant::Value<'_>) -> Value {
    use zbus::zvariant::Value as V;
    match value {
        V::U8(v) => json!(v),
        V::Bool(v) => json!(v),
        V::I16(v) => json!(v),
        V::U16(v) => json!(v),
        V::I32(v) => json!(v),
        V::U32(v) => json!(v),
        V::I64(v) => json!(v),
        V::U64(v) => json!(v),
        V::F64(v) => json!(v),
        V::Str(v) => json!(v.as_str()),
        V::Signature(v) => json!(v.to_string()),
        V::ObjectPath(v) => json!(v.as_str()),
        V::Value(v) => variant_to_json(v),
        V::Array(array) => json!(array.iter().map(variant_to_json).collect::<Vec<_>>()),
        V::Dict(dict) => {
            let mut object = serde_json::Map::new();
            for (key, entry) in dict.iter() {
                let key = match key {
                    V::Str(s) => s.to_string(),
                    other => format!("{other:?}"),
                };
                object.insert(key, variant_to_json(entry));
            }
            Value::Object(object)
        }
        V::Structure(fields) => {
            json!(fields.fields().iter().map(variant_to_json).collect::<Vec<_>>())
        }
        other => json!(format!("{other:?}")),
    }
}

impl ApiHandler for DbusModule {
    fn name(&self) -> &'static str {
        "dbus"
    }

    fn invoke<'a>(&'a self, method: &'a str, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
        Box::pin(async move {
            match method {
                #[cfg(feature = "tier2")]
                "call" => self.call(params).await,
                #[cfg(feature = "tier2")]
                "get" => self.get_property(params).await,
                #[cfg(feature = "tier2")]
                "introspect" => self.introspect(params).await,
                "set" | "ownName" => Err(RpcError::unsupported(
                    "writing D-Bus properties and owning a bus name are not implemented yet",
                )),
                #[cfg(not(feature = "tier2"))]
                "call" | "get" | "introspect" => {
                    let _ = &params;
                    Err(RpcError::unsupported("this build has no D-Bus support"))
                }
                other => Err(RpcError::not_found(&format!("dbus.{other}"))),
            }
        })
    }

    #[cfg(feature = "tier2")]
    fn open_stream<'a>(
        &'a self,
        method: &'a str,
        params: Value,
    ) -> BoxFuture<'a, Result<ValueStream, RpcError>> {
        Box::pin(async move {
            if method != "subscribe" {
                return Err(RpcError::not_found(&format!("dbus.{method}")));
            }
            let params: SubscribeParams = decode("dbus.subscribe", params)?;
            self.ctx.check_dbus(
                params.bus.is_system(),
                params.destination.as_deref().unwrap_or(""),
            )?;

            let connection = self.connect(params.bus).await?;
            let mut rule = zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal);
            if let Some(path) = &params.path {
                rule = rule.path(path.as_str()).map_err(|e| RpcError::invalid_params(e.to_string()))?;
            }
            if let Some(interface) = &params.interface {
                rule = rule.interface(interface.as_str()).map_err(|e| RpcError::invalid_params(e.to_string()))?;
            }
            if let Some(member) = &params.member {
                rule = rule.member(member.as_str()).map_err(|e| RpcError::invalid_params(e.to_string()))?;
            }

            let mut stream = zbus::MessageStream::for_match_rule(rule.build(), &connection, None)
                .await
                .map_err(|e| RpcError::new(htmlapp_bridge::ErrorCode::OperationFailed, e.to_string()))?;

            let signals = async_stream::stream! {
                use futures::StreamExt as _;
                while let Some(Ok(message)) = stream.next().await {
                    let header = message.header();
                    yield Ok(json!({
                        "sender": header.sender().map(|s| s.to_string()),
                        "path": header.path().map(|p| p.to_string()),
                        "iface": header.interface().map(|i| i.to_string()),
                        "member": header.member().map(|m| m.to_string()),
                        "args": Value::Null,
                    }));
                }
            };
            Ok(Box::pin(signals) as ValueStream)
        })
    }
}
