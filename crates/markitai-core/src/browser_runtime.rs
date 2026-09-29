//! Caller-owned Chromium processes and ephemeral web sessions.
use crate::{Result, browser::pool::Pool};

/// A bounded owner for browser reuse within one trusted caller or conversion run.
/// It never shares sessions with another instance or writes reusable login state.
pub struct BrowserRuntime {
    pub(crate) pool: Pool,
}

impl BrowserRuntime {
    /// Capacity counts starting, active, idle and retiring Chromium processes together.
    pub fn new(capacity: usize) -> Result<Self> {
        Ok(Self {
            pool: Pool::new(capacity)?,
        })
    }

    /// Stop admission and dispose idle sessions. Active calls keep their normal
    /// deadlines, then dispose their processes instead of returning them to the pool.
    pub fn close(&self) {
        self.pool.close();
    }
}

impl std::fmt::Debug for BrowserRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrowserRuntime")
            .finish_non_exhaustive()
    }
}
