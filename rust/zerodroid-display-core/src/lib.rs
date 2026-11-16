#![forbid(unsafe_code)]

//! Bounded, platform-neutral metadata validation for Guest display frames.
//!
//! This crate deliberately stops before platform GPU integration. It does not
//! allocate/import buffers, access Android Surface, DRM, EGL or Vulkan, wait on
//! fences, or implement a VirtIO-GPU device. Fence IDs are opaque transport
//! tokens, not file descriptors.

use core::fmt;
use std::collections::VecDeque;

pub const MAX_DAMAGE_RECTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8888,
    Rgbx8888,
    Bgra8888,
    Rgb565,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> u64 {
        match self {
            Self::Rgba8888 | Self::Rgbx8888 | Self::Bgra8888 => 4,
            Self::Rgb565 => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayLimits {
    pub max_width: u32,
    pub max_height: u32,
    pub max_frame_bytes: u64,
}

impl DisplayLimits {
    pub fn validate(self) -> Result<Self, DisplayError> {
        if self.max_width == 0 || self.max_height == 0 || self.max_frame_bytes == 0 {
            return Err(DisplayError::InvalidLimits);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageRect {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

impl DamageRect {
    fn validate(self, frame_width: u32, frame_height: u32) -> Result<(), DisplayError> {
        if self.width == 0 || self.height == 0 {
            return Err(DisplayError::InvalidDamageRect);
        }
        let right = self.left.checked_add(self.width).ok_or(DisplayError::ArithmeticOverflow)?;
        let bottom = self.top.checked_add(self.height).ok_or(DisplayError::ArithmeticOverflow)?;
        if right > frame_width || bottom > frame_height {
            return Err(DisplayError::InvalidDamageRect);
        }
        Ok(())
    }
}

/// A single-plane packed frame descriptor. All sizes and offsets are bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameDescriptor {
    pub buffer_id: u64,
    pub generation: u64,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub offset_bytes: u64,
    pub stride_bytes: u64,
    pub allocation_size_bytes: u64,
    /// Opaque ID interpreted by a platform adapter; never assumed to be an FD.
    pub acquire_fence_id: Option<u64>,
    pub timestamp_ns: u64,
    pub damage: Vec<DamageRect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedFrameLayout {
    pub row_bytes: u64,
    pub payload_bytes: u64,
    pub end_offset_bytes: u64,
}

impl FrameDescriptor {
    pub fn validate(&self, limits: DisplayLimits) -> Result<ValidatedFrameLayout, DisplayError> {
        let limits = limits.validate()?;
        if self.buffer_id == 0 || self.generation == 0 || self.sequence == 0 {
            return Err(DisplayError::InvalidIdentity);
        }
        if self.width == 0 || self.height == 0
            || self.width > limits.max_width || self.height > limits.max_height {
            return Err(DisplayError::InvalidDimensions);
        }
        if self.damage.len() > MAX_DAMAGE_RECTS {
            return Err(DisplayError::TooManyDamageRects);
        }
        for rect in &self.damage {
            rect.validate(self.width, self.height)?;
        }

        let row_bytes = u64::from(self.width)
            .checked_mul(self.format.bytes_per_pixel())
            .ok_or(DisplayError::ArithmeticOverflow)?;
        if self.stride_bytes < row_bytes {
            return Err(DisplayError::InvalidStride);
        }
        let payload_bytes = self.stride_bytes
            .checked_mul(u64::from(self.height))
            .ok_or(DisplayError::ArithmeticOverflow)?;
        if payload_bytes > limits.max_frame_bytes {
            return Err(DisplayError::FrameTooLarge);
        }
        let end_offset_bytes = self.offset_bytes
            .checked_add(payload_bytes)
            .ok_or(DisplayError::ArithmeticOverflow)?;
        if end_offset_bytes > self.allocation_size_bytes {
            return Err(DisplayError::BufferRangeOutOfBounds);
        }
        if self.allocation_size_bytes > limits.max_frame_bytes {
            return Err(DisplayError::FrameTooLarge);
        }
        Ok(ValidatedFrameLayout { row_bytes, payload_bytes, end_offset_bytes })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayError {
    InvalidLimits,
    InvalidIdentity,
    InvalidDimensions,
    InvalidStride,
    InvalidDamageRect,
    TooManyDamageRects,
    ArithmeticOverflow,
    FrameTooLarge,
    BufferRangeOutOfBounds,
    InvalidQueueConfig,
    StaleGeneration,
    NonMonotonicSequence,
    QueueFull,
    QueueByteBudgetExceeded,
    QueueNotEmpty,
    QueueClosed,
}

impl fmt::Display for DisplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for DisplayError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueStats {
    pub queued_frames: usize,
    pub queued_payload_bytes: u64,
    pub generation: u64,
    pub last_accepted_sequence: Option<u64>,
    pub closed: bool,
}

/// A bounded metadata queue. It never copies frame pixels or owns GPU resources.
#[derive(Debug)]
pub struct BoundedFrameQueue {
    frames: VecDeque<(FrameDescriptor, u64)>,
    limits: DisplayLimits,
    generation: u64,
    max_frames: usize,
    max_queued_bytes: u64,
    queued_payload_bytes: u64,
    last_sequence: Option<u64>,
    closed: bool,
}

impl BoundedFrameQueue {
    pub fn new(
        generation: u64,
        max_frames: usize,
        max_queued_bytes: u64,
        limits: DisplayLimits,
    ) -> Result<Self, DisplayError> {
        limits.validate()?;
        if generation == 0 || max_frames == 0 || max_queued_bytes == 0 {
            return Err(DisplayError::InvalidQueueConfig);
        }
        Ok(Self {
            frames: VecDeque::with_capacity(max_frames),
            limits,
            generation,
            max_frames,
            max_queued_bytes,
            queued_payload_bytes: 0,
            last_sequence: None,
            closed: false,
        })
    }

    pub fn push(&mut self, frame: FrameDescriptor) -> Result<(), DisplayError> {
        if self.closed {
            return Err(DisplayError::QueueClosed);
        }
        let layout = frame.validate(self.limits)?;
        if frame.generation != self.generation {
            return Err(DisplayError::StaleGeneration);
        }
        if self.last_sequence.is_some_and(|last| frame.sequence <= last) {
            return Err(DisplayError::NonMonotonicSequence);
        }
        if self.frames.len() >= self.max_frames {
            return Err(DisplayError::QueueFull);
        }
        let next_bytes = self.queued_payload_bytes
            .checked_add(layout.payload_bytes)
            .ok_or(DisplayError::ArithmeticOverflow)?;
        if next_bytes > self.max_queued_bytes {
            return Err(DisplayError::QueueByteBudgetExceeded);
        }
        self.last_sequence = Some(frame.sequence);
        self.queued_payload_bytes = next_bytes;
        self.frames.push_back((frame, layout.payload_bytes));
        Ok(())
    }

    pub fn pop(&mut self) -> Option<FrameDescriptor> {
        let (frame, bytes) = self.frames.pop_front()?;
        self.queued_payload_bytes = self.queued_payload_bytes.saturating_sub(bytes);
        Some(frame)
    }

    /// Changes the Surface/VM generation only after all old frames are drained.
    pub fn reset_generation(&mut self, generation: u64) -> Result<(), DisplayError> {
        if self.closed {
            return Err(DisplayError::QueueClosed);
        }
        if generation == 0 || generation == self.generation {
            return Err(DisplayError::InvalidQueueConfig);
        }
        if !self.frames.is_empty() {
            return Err(DisplayError::QueueNotEmpty);
        }
        self.generation = generation;
        self.last_sequence = None;
        Ok(())
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.frames.clear();
        self.queued_payload_bytes = 0;
    }

    pub fn stats(&self) -> QueueStats {
        QueueStats {
            queued_frames: self.frames.len(),
            queued_payload_bytes: self.queued_payload_bytes,
            generation: self.generation,
            last_accepted_sequence: self.last_sequence,
            closed: self.closed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> DisplayLimits {
        DisplayLimits { max_width: 4096, max_height: 4096, max_frame_bytes: 64 * 1024 * 1024 }
    }

    fn frame(sequence: u64) -> FrameDescriptor {
        FrameDescriptor {
            buffer_id: 7,
            generation: 1,
            sequence,
            width: 100,
            height: 50,
            format: PixelFormat::Rgba8888,
            offset_bytes: 128,
            stride_bytes: 400,
            allocation_size_bytes: 20_128,
            acquire_fence_id: Some(99),
            timestamp_ns: 1234,
            damage: vec![DamageRect { left: 0, top: 0, width: 100, height: 50 }],
        }
    }

    #[test]
    fn validates_packed_frame_and_checked_buffer_range() {
        let layout = frame(1).validate(limits()).unwrap();
        assert_eq!(layout.row_bytes, 400);
        assert_eq!(layout.payload_bytes, 20_000);
        assert_eq!(layout.end_offset_bytes, 20_128);
    }

    #[test]
    fn rejects_stride_smaller_than_pixel_row() {
        let mut value = frame(1);
        value.stride_bytes = 399;
        assert_eq!(value.validate(limits()), Err(DisplayError::InvalidStride));
    }

    #[test]
    fn rejects_out_of_bounds_or_overflowing_buffer_range() {
        let mut value = frame(1);
        value.allocation_size_bytes = 20_127;
        assert_eq!(value.validate(limits()), Err(DisplayError::BufferRangeOutOfBounds));
        value.offset_bytes = u64::MAX - 10;
        assert_eq!(value.validate(limits()), Err(DisplayError::ArithmeticOverflow));
    }

    #[test]
    fn rejects_damage_rectangles_outside_frame() {
        let mut value = frame(1);
        value.damage = vec![DamageRect { left: 99, top: 0, width: 2, height: 1 }];
        assert_eq!(value.validate(limits()), Err(DisplayError::InvalidDamageRect));
    }

    #[test]
    fn queue_is_bounded_and_backpressured_without_partial_acceptance() {
        let mut queue = BoundedFrameQueue::new(1, 2, 40_000, limits()).unwrap();
        queue.push(frame(1)).unwrap();
        queue.push(frame(2)).unwrap();
        assert_eq!(queue.push(frame(3)), Err(DisplayError::QueueFull));
        assert_eq!(queue.stats().queued_payload_bytes, 40_000);
        assert_eq!(queue.pop().unwrap().sequence, 1);
        queue.push(frame(3)).unwrap();
        assert_eq!(queue.stats().queued_frames, 2);
    }

    #[test]
    fn queue_rejects_stale_and_non_monotonic_frames() {
        let mut queue = BoundedFrameQueue::new(1, 2, 40_000, limits()).unwrap();
        let mut stale = frame(1);
        stale.generation = 2;
        assert_eq!(queue.push(stale), Err(DisplayError::StaleGeneration));
        queue.push(frame(2)).unwrap();
        assert_eq!(queue.push(frame(2)), Err(DisplayError::NonMonotonicSequence));
    }

    #[test]
    fn generation_can_change_only_after_queue_is_drained() {
        let mut queue = BoundedFrameQueue::new(1, 2, 40_000, limits()).unwrap();
        queue.push(frame(1)).unwrap();
        assert_eq!(queue.reset_generation(2), Err(DisplayError::QueueNotEmpty));
        queue.pop();
        queue.reset_generation(2).unwrap();
        let mut next = frame(1);
        next.generation = 2;
        queue.push(next).unwrap();
    }

    #[test]
    fn close_releases_metadata_and_rejects_future_frames() {
        let mut queue = BoundedFrameQueue::new(1, 2, 40_000, limits()).unwrap();
        queue.push(frame(1)).unwrap();
        queue.close();
        assert_eq!(queue.stats().queued_frames, 0);
        assert_eq!(queue.stats().queued_payload_bytes, 0);
        assert_eq!(queue.push(frame(2)), Err(DisplayError::QueueClosed));
    }

    #[test]
    fn rejects_frame_over_budget() {
        let mut value = frame(1);
        value.width = 4096;
        value.height = 4096;
        value.stride_bytes = 4096 * 4;
        value.allocation_size_bytes = u64::MAX;
        assert_eq!(value.validate(limits()), Err(DisplayError::FrameTooLarge));
    }
}
