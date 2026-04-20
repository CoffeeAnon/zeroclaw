//! Durable outbox for A2A push-notification delivery.
//!
//! Implements [`ra2a::PushNotificationSender`] with at-least-once delivery
//! semantics backed by a Postgres `outbox` table. Failed POSTs are retried
//! with exponential backoff; after `max_attempts` the row moves to
//! dead-letter state and is surfaced via a structured log event.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
