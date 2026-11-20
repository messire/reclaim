//! Reclaim engine: the worker thread, command/event protocol, run state machine,
//! follow-up sampling and local persistence (ARCHITECTURE §4, §7, §9).
//!
//! The engine contains no OS calls and no toolkit types: platform access goes through the
//! ports of `reclaim-core`, so the same engine runs over the Windows adapter and the fake.

pub mod coordinator;
pub mod engine;
pub mod store;

pub use coordinator::{Coordinator, Phase, Platform, Row, Step};
pub use engine::{Command, EngineHandle, Event, Finished, StdClock, View, spawn};
pub use store::{Settings, Store};
