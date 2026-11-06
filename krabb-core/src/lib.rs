#![forbid(unsafe_code)]
#![allow(missing_docs)]

pub mod agent;
pub mod error;
pub mod jsonl;
pub mod plugin;
pub mod policy;
pub mod schedule;
pub mod scope;
pub mod session;
pub mod types;

pub use error::{Error, Result};
pub use types::{Capability, Content, Event, Message, Role};

pub mod visibility;
