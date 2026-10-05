//! Physical screen geometry and a single screenshot intent, held until its receipt.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub const MAX_PIXEL_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
impl Rect {
    pub fn dimensions(self) -> Result<(u32, u32), String> {
        let width = i64::from(self.right) - i64::from(self.left);
        let height = i64::from(self.bottom) - i64::from(self.top);
        if width <= 0 || height <= 0 {
            return Err("截图选区为空".into());
        }
        Ok((
            u32::try_from(width).map_err(|_| "截图宽度溢出")?,
            u32::try_from(height).map_err(|_| "截图高度溢出")?,
        ))
    }
}
pub fn pixel_bytes(width: u32, height: u32) -> Result<usize, String> {
    if width == 0 || height == 0 {
        return Err("截图选区为空".into());
    }
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|n| n.checked_mul(4))
        .filter(|n| *n <= MAX_PIXEL_BYTES as u64)
        .map(|n| n as usize)
        .ok_or_else(|| "截图原始像素超过 64 MiB 上限".into())
}
pub fn selection(
    start: (i32, i32),
    end: (i32, i32),
    desktop: Rect,
) -> Result<Option<Rect>, String> {
    desktop.dimensions()?;
    let rect = Rect {
        left: start.0.min(end.0).max(desktop.left),
        top: start.1.min(end.1).max(desktop.top),
        right: start.0.max(end.0).min(desktop.right),
        bottom: start.1.max(end.1).min(desktop.bottom),
    };
    if rect.left >= rect.right || rect.top >= rect.bottom {
        return Ok(None);
    }
    let (width, height) = rect.dimensions()?;
    pixel_bytes(width, height)?;
    Ok(Some(rect))
}

#[derive(Clone)]
pub struct ScreenshotTicket {
    pub generation: u64,
    pub expected_sequence: u32,
    current: Arc<AtomicBool>,
}
impl ScreenshotTicket {
    pub fn current(&self) -> bool {
        self.current.load(Ordering::Acquire)
    }
    pub fn may_write(&self, sequence: u32) -> bool {
        self.current() && sequence == self.expected_sequence
    }
}
#[derive(Default)]
pub struct ScreenshotGate {
    generation: u64,
    active: Option<(ScreenshotTicket, bool)>,
    closed: bool,
}
impl ScreenshotGate {
    pub fn begin(&mut self, sequence: u32) -> Option<ScreenshotTicket> {
        if self.closed || self.busy() {
            return None;
        }
        self.generation = self.generation.wrapping_add(1);
        let ticket = ScreenshotTicket {
            generation: self.generation,
            expected_sequence: sequence,
            current: Arc::new(AtomicBool::new(true)),
        };
        self.active = Some((ticket.clone(), false));
        Some(ticket)
    }
    pub fn encoding(&mut self, ticket: &ScreenshotTicket) -> bool {
        if let Some((active, encoding)) = &mut self.active
            && active.generation == ticket.generation
            && active.current()
            && !*encoding
        {
            *encoding = true;
            return true;
        }
        false
    }
    pub fn cancel(&mut self) {
        if let Some((ticket, _)) = &self.active {
            ticket.current.store(false, Ordering::Release);
        }
    }
    pub fn close(&mut self) {
        self.closed = true;
        self.cancel();
    }
    /// Cancelled shutdown permits fresh intents, never revives a revoked ticket.
    pub fn resume(&mut self) {
        self.closed = false;
    }
    pub fn busy(&self) -> bool {
        self.active.is_some()
    }
    pub fn finish(&mut self, ticket: &ScreenshotTicket) -> bool {
        if self
            .active
            .as_ref()
            .is_some_and(|(active, _)| active.generation == ticket.generation)
        {
            self.cancel();
            self.active = None;
            return true;
        }
        false
    }
}
