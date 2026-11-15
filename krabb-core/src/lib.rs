#![forbid(unsafe_code)]
#![allow(missing_docs)]

pub mod agent;
pub mod client;
pub mod context;
pub mod error;
pub mod execution;
pub mod failure;
pub mod jsonl;
pub mod operation;
pub mod plugin;
pub mod policy;
pub mod reply;
pub mod schedule;
pub mod scope;
pub mod session;
pub mod task;
pub mod types;

pub use error::{Error, Result};
pub use types::{Capability, Content, Event, Message, Role};

pub mod visibility;
