pub mod registry;
pub mod schema;

pub use registry::{Adapter, CommandId, CommandKind, CommandMetadata, CommandRegistry};
pub use schema::{CommandError, CommandResult};
