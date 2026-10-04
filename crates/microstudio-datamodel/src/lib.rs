// mutations return PendingEvents; caller fires after borrow release, so listeners may re-enter

pub mod api;
pub mod attributes;
pub mod classes;
mod datamodel;
mod instance;
mod signal;

pub use attributes::AttributeValue;
pub use classes::{ClassDescriptor, ClassKind};
pub use datamodel::{
    class_kind, default_property_value, property_type, DataModel, DataModelError, PendingEvent,
    PropertyType, DEFAULT_SERVICES,
};
pub use instance::{EventKind, InstanceData, InstanceId};
pub use signal::{Connection, ConnectionId, EventArg, Signal, SignalId};
