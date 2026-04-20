//! Durable outbox for A2A push-notification delivery.
//!
//! Implements [`ra2a::PushNotificationSender`] with at-least-once delivery
//! semantics backed by a Postgres `outbox` table. Failed POSTs are retried
//! with exponential backoff; after `max_attempts` the row moves to
//! dead-letter state and is surfaced via a structured log event.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

pub mod migrate;
pub mod push_sender;
pub mod record;
pub mod retry;
pub mod store;
pub mod worker;
pub use push_sender::OutboxBackedPushSender;
pub use record::OutboxRecord;
pub use retry::RetryPolicy;
pub use store::OutboxStore;
pub use worker::OutboxWorker;
