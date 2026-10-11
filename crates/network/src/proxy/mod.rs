pub mod model;
pub mod parser;

pub use model::{ProxyProtocol, ProxyRow};
pub use parser::{ParseReport, ParseRowError, parse_lines};
pub mod assignment;
pub mod health;
pub mod repository;

pub use assignment::{
    AssignmentDelete, AssignmentError, AssignmentUpdate, ProxyAssignment, ProxyAssignmentRepository,
};
pub use repository::{
    ProxyCreateResult, ProxyDelete, ProxyRecord, ProxyRepository, ProxyRepositorySnapshot,
    ProxySnapshotRecord, ProxyUpdate, RepositoryError,
};
