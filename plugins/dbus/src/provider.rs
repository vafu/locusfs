use async_trait::async_trait;
use locusfs_graph::{
    GraphError, GraphPathChild, GraphPathDirectory, GraphPathEntry, LocusValue, NodeAccess, NodeId,
    NodeKind, NodeProvider, PathName, PathProvider, PropertyKey, PropertyMutationProvider,
    PropertyProvider, PropertySpec, RelationName, RelationProvider, Result,
};
use tokio::runtime::Handle;
use zbus::names::InterfaceName;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, StructureBuilder, Value};
use zbus::{Connection, fdo::PropertiesProxy};

use crate::DBUS_METHOD_KIND;
use crate::state::{BusKind, DbusCallableMethod, DbusState, DbusWritableProperty, SharedDbusState};

#[derive(Clone)]
pub struct DbusProvider {
    kind: NodeKind,
    state: SharedDbusState,
    runtime: Handle,
}

impl DbusProvider {
    pub(crate) fn new(kind: NodeKind, state: SharedDbusState, runtime: Handle) -> Self {
        Self {
            kind,
            state,
            runtime,
        }
    }

    async fn with_state<T>(&self, operation: impl FnOnce(&DbusState) -> Result<T>) -> Result<T> {
        let state = self.state.read().await;
        operation(&state)
    }
}

#[async_trait]
impl NodeProvider for DbusProvider {
    fn kind(&self) -> &NodeKind {
        &self.kind
    }

    fn access(&self) -> NodeAccess {
        if matches!(
            self.kind.as_str(),
            crate::DBUS_OBJECT_KIND | crate::DBUS_METHOD_KIND
        ) {
            NodeAccess::hidden()
        } else {
            NodeAccess::read_only()
        }
    }

    async fn contains_node(&self, node: &NodeId) -> Result<bool> {
        self.with_state(|state| state.contains_node(node)).await
    }

    async fn nodes(&self) -> Result<Vec<NodeId>> {
        self.with_state(|state| state.nodes(&self.kind)).await
    }
}

#[async_trait]
impl PropertyProvider for DbusProvider {
    async fn property_spec(&self, subject: &NodeId, key: &PropertyKey) -> Result<PropertySpec> {
        self.with_state(|state| state.property_spec(subject, key))
            .await
    }

    async fn properties(&self, subject: &NodeId) -> Result<Vec<PropertySpec>> {
        self.with_state(|state| state.properties(subject)).await
    }

    async fn property(&self, subject: &NodeId, key: &PropertyKey) -> Result<LocusValue> {
        self.with_state(|state| state.property(subject, key)).await
    }
}

#[async_trait]
impl RelationProvider for DbusProvider {
    async fn relations(&self, source: &NodeId) -> Result<Vec<RelationName>> {
        self.with_state(|state| state.relations(source)).await
    }

    async fn targets(&self, source: &NodeId, relation: &RelationName) -> Result<Vec<NodeId>> {
        self.with_state(|state| state.targets(source, relation))
            .await
    }
}

#[async_trait]
impl PathProvider for DbusProvider {
    fn kind(&self) -> &NodeKind {
        &self.kind
    }

    async fn lookup_child(
        &self,
        parent: &GraphPathDirectory,
        name: &PathName,
    ) -> Result<Option<GraphPathEntry>> {
        self.with_state(|state| state.path_lookup_child(parent, name))
            .await
    }

    async fn children(&self, parent: &GraphPathDirectory) -> Result<Option<Vec<GraphPathChild>>> {
        self.with_state(|state| state.path_children(parent)).await
    }

    async fn watch_target(
        &self,
        directory: &GraphPathDirectory,
    ) -> Result<Option<locusfs_graph::GraphWatchTarget>> {
        self.with_state(|state| state.path_watch_target(directory))
            .await
    }
}

#[async_trait]
impl PropertyMutationProvider for DbusProvider {
    async fn set_property(
        &self,
        subject: &NodeId,
        key: &PropertyKey,
        value: LocusValue,
    ) -> Result<()> {
        if subject.kind().as_str() == DBUS_METHOD_KIND {
            let callable = self
                .with_state(|state| state.callable_method(subject, key))
                .await?;
            let LocusValue::String(arguments) = value else {
                return Err(GraphError::InvalidValue {
                    kind: "D-Bus method call arguments",
                    value: value.to_string(),
                    reason: "expected comma-separated string",
                });
            };
            self.runtime
                .spawn(call_dbus_method(callable, arguments))
                .await
                .map_err(|error| {
                    GraphError::Io(format!("call D-Bus method task failed: {error}"))
                })??;
            return Ok(());
        }

        let writable = self
            .with_state(|state| state.writable_property(subject, key))
            .await?;
        self.runtime
            .spawn(set_dbus_property(writable, value.clone()))
            .await
            .map_err(|error| {
                GraphError::Io(format!("set D-Bus property task failed: {error}"))
            })??;
        let mut state = self.state.write().await;
        if let Err(error) = state.update_cached_property(subject, key, value) {
            eprintln!("locusfs-dbus: failed to update cached D-Bus property after write: {error}");
        }
        Ok(())
    }

    async fn remove_property(&self, subject: &NodeId, key: &PropertyKey) -> Result<()> {
        Err(GraphError::InvalidValue {
            kind: "D-Bus property",
            value: format!("{subject}/{key}"),
            reason: "property removal is not supported by D-Bus Properties",
        })
    }
}

async fn call_dbus_method(target: DbusCallableMethod, arguments: String) -> Result<()> {
    let connection = connection(target.service.bus).await?;
    let args = parse_method_arguments(&target.input_signature, &arguments)?;
    call_with_arguments(&connection, target, args).await
}

async fn set_dbus_property(target: DbusWritableProperty, value: LocusValue) -> Result<()> {
    let connection = connection(target.service.bus).await?;
    let proxy = PropertiesProxy::builder(&connection)
        .destination(target.service.name.as_str())
        .map_err(|error| GraphError::Io(format!("create properties destination: {error}")))?
        .path(target.object_path.as_str())
        .map_err(|error| {
            GraphError::Io(format!(
                "create properties path {}: {error}",
                target.object_path
            ))
        })?
        .build()
        .await
        .map_err(|error| {
            GraphError::Io(format!(
                "create properties proxy for {} at {}: {error}",
                target.service.name, target.object_path
            ))
        })?;
    let interface = InterfaceName::try_from(target.interface.as_str()).map_err(|_| {
        GraphError::InvalidValue {
            kind: "D-Bus interface",
            value: target.interface.clone(),
            reason: "invalid interface name",
        }
    })?;

    if value.kind() != target.current.kind() {
        return Err(GraphError::InvalidValue {
            kind: "D-Bus property value",
            value: value.to_string(),
            reason: "value kind does not match the current D-Bus property kind",
        });
    }

    match value {
        LocusValue::String(value) => {
            proxy
                .set(interface, &target.property, Value::new(value.as_str()))
                .await
        }
        LocusValue::Bool(value) => {
            proxy
                .set(interface, &target.property, Value::new(value))
                .await
        }
        LocusValue::U32(value) => {
            proxy
                .set(interface, &target.property, Value::new(value))
                .await
        }
        LocusValue::I32(value) => {
            proxy
                .set(interface, &target.property, Value::new(value))
                .await
        }
        LocusValue::F64(value) => {
            proxy
                .set(interface, &target.property, Value::new(value))
                .await
        }
    }
    .map_err(|error| {
        GraphError::Io(format!(
            "set D-Bus property {}.{} on {} at {}: {error}",
            target.interface, target.property, target.service.name, target.object_path
        ))
    })
}

async fn connection(bus: BusKind) -> Result<Connection> {
    match bus {
        BusKind::System => Connection::system()
            .await
            .map_err(|error| GraphError::Io(format!("connect to system D-Bus: {error}"))),
        BusKind::Session => Connection::session()
            .await
            .map_err(|error| GraphError::Io(format!("connect to session D-Bus: {error}"))),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum MethodArgument {
    String(String),
    Bool(bool),
    U32(u32),
    I32(i32),
    F64(f64),
    ObjectPath(String),
}

fn parse_method_arguments(signatures: &[String], input: &str) -> Result<Vec<MethodArgument>> {
    let input = input.trim_end_matches(['\r', '\n']);
    let raw_args = if input.trim().is_empty() {
        Vec::new()
    } else {
        input.split(',').map(str::trim).collect::<Vec<_>>()
    };
    if raw_args.len() != signatures.len() {
        return Err(GraphError::InvalidValue {
            kind: "D-Bus method call arguments",
            value: input.to_string(),
            reason: "argument count does not match method input signature",
        });
    }

    signatures
        .iter()
        .zip(raw_args)
        .map(|(signature, value)| parse_method_argument(signature, value))
        .collect()
}

fn parse_method_argument(signature: &str, value: &str) -> Result<MethodArgument> {
    match signature {
        "s" => Ok(MethodArgument::String(value.to_string())),
        "b" => match value {
            "true" | "1" => Ok(MethodArgument::Bool(true)),
            "false" | "0" => Ok(MethodArgument::Bool(false)),
            _ => Err(GraphError::InvalidValue {
                kind: "D-Bus bool argument",
                value: value.to_string(),
                reason: "expected true, false, 1, or 0",
            }),
        },
        "u" => {
            value
                .parse::<u32>()
                .map(MethodArgument::U32)
                .map_err(|_| GraphError::InvalidValue {
                    kind: "D-Bus u32 argument",
                    value: value.to_string(),
                    reason: "expected unsigned integer",
                })
        }
        "i" => {
            value
                .parse::<i32>()
                .map(MethodArgument::I32)
                .map_err(|_| GraphError::InvalidValue {
                    kind: "D-Bus i32 argument",
                    value: value.to_string(),
                    reason: "expected signed integer",
                })
        }
        "d" => {
            let number = value.parse::<f64>().map_err(|_| GraphError::InvalidValue {
                kind: "D-Bus f64 argument",
                value: value.to_string(),
                reason: "expected float",
            })?;
            if !number.is_finite() {
                return Err(GraphError::InvalidValue {
                    kind: "D-Bus f64 argument",
                    value: value.to_string(),
                    reason: "expected finite float",
                });
            }
            Ok(MethodArgument::F64(number))
        }
        "o" => ObjectPath::try_from(value)
            .map(|_| MethodArgument::ObjectPath(value.to_string()))
            .map_err(|_| GraphError::InvalidValue {
                kind: "D-Bus object path argument",
                value: value.to_string(),
                reason: "invalid object path",
            }),
        _ => Err(GraphError::InvalidValue {
            kind: "D-Bus method argument signature",
            value: signature.to_string(),
            reason: "unsupported argument type",
        }),
    }
}

async fn call_with_arguments(
    connection: &Connection,
    target: DbusCallableMethod,
    args: Vec<MethodArgument>,
) -> Result<()> {
    let destination = target.service.name.as_str();
    let path = target.object_path.as_str();
    let interface = target.interface.as_str();
    let method = target.method.as_str();
    let result = if args.is_empty() {
        connection
            .call_method(Some(destination), path, Some(interface), method, &())
            .await
    } else {
        let body = method_body(args)?;
        connection
            .call_method(Some(destination), path, Some(interface), method, &body)
            .await
    };

    result.map(|_| ()).map_err(|error| {
        GraphError::Io(format!(
            "call D-Bus method {}.{} on {} at {}: {error}",
            target.interface, target.method, target.service.name, target.object_path
        ))
    })
}

fn method_body(args: Vec<MethodArgument>) -> Result<zbus::zvariant::Structure<'static>> {
    let mut builder = StructureBuilder::new();
    for arg in args {
        match arg {
            MethodArgument::String(value) => builder.push_field(value),
            MethodArgument::Bool(value) => builder.push_field(value),
            MethodArgument::U32(value) => builder.push_field(value),
            MethodArgument::I32(value) => builder.push_field(value),
            MethodArgument::F64(value) => builder.push_field(value),
            MethodArgument::ObjectPath(value) => {
                let value =
                    OwnedObjectPath::try_from(value).map_err(|error| GraphError::InvalidValue {
                        kind: "D-Bus object path argument",
                        value: error.to_string(),
                        reason: "invalid object path",
                    })?;
                builder.push_field(value);
            }
        }
    }
    builder.build().map_err(|error| GraphError::InvalidValue {
        kind: "D-Bus method call arguments",
        value: error.to_string(),
        reason: "failed to build D-Bus message body",
    })
}
