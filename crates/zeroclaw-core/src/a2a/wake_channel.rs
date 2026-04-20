//! Signal channel used by the webhook receiver to nudge Sam's reasoning loop.
//!
//! The reasoning loop should `tokio::select!` on `receiver.recv()` alongside
//! its existing trigger sources. A received `WakeSignal` means there are one
//! or more unprocessed rows in `sam.inbox_events`.

use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy)]
pub struct WakeSignal;

#[derive(Clone)]
pub struct WakeSender(mpsc::UnboundedSender<WakeSignal>);

pub struct WakeReceiver(mpsc::UnboundedReceiver<WakeSignal>);

pub fn channel() -> (WakeSender, WakeReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    (WakeSender(tx), WakeReceiver(rx))
}

impl WakeSender {
    /// Non-blocking wake. Silently drops if the receiver has been closed
    /// (loop has shut down; wake no longer meaningful).
    pub fn wake(&self) {
        let _ = self.0.send(WakeSignal);
    }
}

impl WakeReceiver {
    pub async fn recv(&mut self) -> Option<WakeSignal> {
        self.0.recv().await
    }

    pub fn try_recv(&mut self) -> Result<WakeSignal, mpsc::error::TryRecvError> {
        self.0.try_recv()
    }
}
