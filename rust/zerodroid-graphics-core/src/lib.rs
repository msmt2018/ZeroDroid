#![forbid(unsafe_code)]

//! Independent, platform-neutral graphics contracts for ZeroDroid.
//!
//! This crate validates display geometry, pixel-buffer layouts, color metadata,
//! damage regions and bounded frame scheduling. It performs no GPU calls,
//! Android Surface access, DMA-BUF import, EGL/Vulkan work or pixel conversion.

use core::fmt;

pub const MAX_DISPLAY_DIMENSION: u32 = 16_384;
pub const MAX_BUFFER_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_DAMAGE_RECTS: usize = 64;
pub const MAX_QUEUED_FRAMES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsError {
    InvalidDimensions, InvalidDensity, InvalidRefreshRate, UnsupportedScale,
    ArithmeticOverflow, InvalidStride, InvalidBufferRange, BufferTooLarge,
    InvalidDamage, TooManyDamageRects, InvalidGeneration, InvalidSequence,
    InvalidQueueCapacity, QueueFull, QueueEmpty, DuplicateBuffer, UnknownBuffer,
    BufferBusy, InvalidBufferState, InvalidBufferId, LeaseIdExhausted, InvalidBufferId, InvalidFenceId, LeaseIdExhausted,
}
impl fmt::Display for GraphicsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{self:?}") }
}
impl std::error::Error for GraphicsError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat { Rgba8888, Rgbx8888, Bgra8888, Rgb565, Rgba1010102, RgbaFp16 }
impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> u32 {
        match self { Self::Rgb565 => 2, Self::RgbaFp16 => 8,
            Self::Rgba8888 | Self::Rgbx8888 | Self::Bgra8888 | Self::Rgba1010102 => 4 }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace { Srgb, DisplayP3, Bt2020, LinearSrgb, ScRgb }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaMode { Opaque, Premultiplied, Unpremultiplied }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation { R0, R90, R180, R270 }

/// A backend must never silently choose a resampling mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleMode {
    /// Guest pixels map one-to-one to physical output pixels.
    OneToOne,
    /// Only integer enlargement, using nearest-neighbor sampling.
    IntegerNearest,
    /// Filtering is permitted only when explicitly selected by the caller.
    Filtered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayGeometry {
    pub guest_width: u32, pub guest_height: u32,
    pub output_width: u32, pub output_height: u32,
    pub density_dpi: u32,
    /// Millihertz; 120_000 represents 120 Hz.
    pub refresh_rate_millihz: u32,
    pub rotation: Rotation,
    pub scale_mode: ScaleMode,
}
impl DisplayGeometry {
    pub fn validate(self) -> Result<Self, GraphicsError> {
        for dimension in [self.guest_width, self.guest_height, self.output_width, self.output_height] {
            if dimension == 0 || dimension > MAX_DISPLAY_DIMENSION { return Err(GraphicsError::InvalidDimensions); }
        }
        if self.density_dpi == 0 || self.density_dpi > 2_000 { return Err(GraphicsError::InvalidDensity); }
        if !(1_000..=240_000).contains(&self.refresh_rate_millihz) {
            return Err(GraphicsError::InvalidRefreshRate);
        }
        let (out_w, out_h) = match self.rotation {
            Rotation::R0 | Rotation::R180 => (self.output_width, self.output_height),
            Rotation::R90 | Rotation::R270 => (self.output_height, self.output_width),
        };
        match self.scale_mode {
            ScaleMode::OneToOne if self.guest_width != out_w || self.guest_height != out_h =>
                return Err(GraphicsError::UnsupportedScale),
            ScaleMode::IntegerNearest => {
                if out_w < self.guest_width || out_h < self.guest_height
                    || out_w % self.guest_width != 0 || out_h % self.guest_height != 0
                    || out_w / self.guest_width != out_h / self.guest_height {
                    return Err(GraphicsError::UnsupportedScale);
                }
            }
            ScaleMode::Filtered | ScaleMode::OneToOne => {}
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferLayout {
    pub width: u32, pub height: u32, pub stride_bytes: u32,
    pub format: PixelFormat, pub offset_bytes: u64, pub allocation_bytes: u64,
}
impl BufferLayout {
    pub fn validate(self) -> Result<Self, GraphicsError> {
        if self.width == 0 || self.height == 0
            || self.width > MAX_DISPLAY_DIMENSION || self.height > MAX_DISPLAY_DIMENSION {
            return Err(GraphicsError::InvalidDimensions);
        }
        let row_bytes = self.width.checked_mul(self.format.bytes_per_pixel())
            .ok_or(GraphicsError::ArithmeticOverflow)?;
        if self.stride_bytes < row_bytes || self.stride_bytes % self.format.bytes_per_pixel() != 0 {
            return Err(GraphicsError::InvalidStride);
        }
        let required = u64::from(self.stride_bytes).checked_mul(u64::from(self.height))
            .ok_or(GraphicsError::ArithmeticOverflow)?;
        let end = self.offset_bytes.checked_add(required).ok_or(GraphicsError::ArithmeticOverflow)?;
        if required == 0 || end > self.allocation_bytes { return Err(GraphicsError::InvalidBufferRange); }
        if self.allocation_bytes > MAX_BUFFER_BYTES { return Err(GraphicsError::BufferTooLarge); }
        Ok(self)
    }
    pub fn row_bytes(self) -> Result<u32, GraphicsError> {
        self.width.checked_mul(self.format.bytes_per_pixel()).ok_or(GraphicsError::ArithmeticOverflow)
    }
    pub fn required_bytes(self) -> Result<u64, GraphicsError> {
        u64::from(self.stride_bytes).checked_mul(u64::from(self.height))
            .ok_or(GraphicsError::ArithmeticOverflow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect { pub left: u32, pub top: u32, pub right: u32, pub bottom: u32 }
impl Rect {
    pub fn validate(self, width: u32, height: u32) -> Result<Self, GraphicsError> {
        if self.left >= self.right || self.top >= self.bottom
            || self.right > width || self.bottom > height { return Err(GraphicsError::InvalidDamage); }
        Ok(self)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DamageRegion { pub rects: Vec<Rect> }
impl DamageRegion {
    pub fn validate(&self, width: u32, height: u32) -> Result<(), GraphicsError> {
        if self.rects.len() > MAX_DAMAGE_RECTS { return Err(GraphicsError::TooManyDamageRects); }
        for rect in &self.rects { rect.validate(width, height)?; }
        Ok(())
    }
    pub fn is_full_frame(&self, width: u32, height: u32) -> bool {
        self.rects.as_slice() == [Rect { left: 0, top: 0, right: width, bottom: height }]
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferId(pub u64);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FenceId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameDescriptor {
    pub display_generation: u64, pub sequence: u64, pub buffer_id: BufferId,
    pub acquire_fence: Option<FenceId>, pub damage: DamageRegion,
    pub color_space: ColorSpace, pub alpha_mode: AlphaMode,
}
impl FrameDescriptor {
    pub fn validate(&self, layout: BufferLayout) -> Result<(), GraphicsError> {
        layout.validate()?;
        if self.display_generation == 0 { return Err(GraphicsError::InvalidGeneration); }
        if self.sequence == 0 { return Err(GraphicsError::InvalidSequence); }
        self.damage.validate(layout.width, layout.height)
    }
}

#[derive(Debug)]
pub struct FrameQueue {
    capacity: usize,
    frames: std::collections::VecDeque<FrameDescriptor>,
    last_sequence: u64,
    generation: u64,
}
impl FrameQueue {
    pub fn new(capacity: usize, generation: u64) -> Result<Self, GraphicsError> {
        if capacity == 0 || capacity > MAX_QUEUED_FRAMES { return Err(GraphicsError::InvalidQueueCapacity); }
        if generation == 0 { return Err(GraphicsError::InvalidGeneration); }
        Ok(Self { capacity, frames: std::collections::VecDeque::with_capacity(capacity),
            last_sequence: 0, generation })
    }
    pub fn len(&self) -> usize { self.frames.len() }
    pub fn is_empty(&self) -> bool { self.frames.is_empty() }
    pub fn push(&mut self, frame: FrameDescriptor, layout: BufferLayout) -> Result<(), GraphicsError> {
        frame.validate(layout)?;
        if frame.display_generation != self.generation { return Err(GraphicsError::InvalidGeneration); }
        if frame.sequence <= self.last_sequence { return Err(GraphicsError::InvalidSequence); }
        if self.frames.len() >= self.capacity { return Err(GraphicsError::QueueFull); }
        self.last_sequence = frame.sequence;
        self.frames.push_back(frame);
        Ok(())
    }
    pub fn pop(&mut self) -> Result<FrameDescriptor, GraphicsError> {
        self.frames.pop_front().ok_or(GraphicsError::QueueEmpty)
    }
    pub fn reset_generation(&mut self, generation: u64) -> Result<(), GraphicsError> {
        if generation <= self.generation { return Err(GraphicsError::InvalidGeneration); }
        self.frames.clear();
        self.generation = generation;
        self.last_sequence = 0;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferRegistration {
    pub id: BufferId, pub layout: BufferLayout,
    pub color_space: ColorSpace, pub alpha_mode: AlphaMode,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferLease { pub id: BufferId, pub generation: u64, pub lease_id: u64 }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BufferState { Available, Queued(u64), InFlight(u64) }

/// Bounded metadata registry. Pixel allocations remain owned by GPU/platform backends.
#[derive(Debug)]
pub struct BufferRegistry {
    generation: u64,
    entries: std::collections::BTreeMap<u64, (BufferRegistration, BufferState)>,
    max_entries: usize,
    next_lease_id: u64,
}
impl BufferRegistry {
    pub fn new(generation: u64, max_entries: usize) -> Result<Self, GraphicsError> {
        if generation == 0 { return Err(GraphicsError::InvalidGeneration); }
        if max_entries == 0 || max_entries > 256 { return Err(GraphicsError::InvalidQueueCapacity); }
        Ok(Self { generation, entries: std::collections::BTreeMap::new(), max_entries, next_lease_id: 1 })
    }
    pub fn register(&mut self, registration: BufferRegistration) -> Result<(), GraphicsError> {
        registration.layout.validate()?;
        if registration.id.0 == 0 { return Err(GraphicsError::InvalidBufferId); }
        if self.entries.contains_key(&registration.id.0) { return Err(GraphicsError::DuplicateBuffer); }
        if self.entries.len() >= self.max_entries { return Err(GraphicsError::QueueFull); }
        self.entries.insert(registration.id.0, (registration, BufferState::Available));
        Ok(())
    }
    pub fn acquire(&mut self, id: BufferId) -> Result<BufferLease, GraphicsError> {
        let (_, state) = self.entries.get_mut(&id.0).ok_or(GraphicsError::UnknownBuffer)?;
        if *state != BufferState::Available { return Err(GraphicsError::BufferBusy); }
        let lease_id = self.next_lease_id;
        self.next_lease_id = self.next_lease_id.checked_add(1).ok_or(GraphicsError::LeaseIdExhausted)?;
        *state = BufferState::Queued(lease_id);
        Ok(BufferLease { id, generation: self.generation, lease_id })
    }
    pub fn mark_in_flight(&mut self, lease: BufferLease) -> Result<(), GraphicsError> {
        if lease.generation != self.generation { return Err(GraphicsError::InvalidGeneration); }
        let (_, state) = self.entries.get_mut(&lease.id.0).ok_or(GraphicsError::UnknownBuffer)?;
        if *state != BufferState::Queued(lease.lease_id) { return Err(GraphicsError::InvalidBufferState); }
        *state = BufferState::InFlight(lease.lease_id);
        Ok(())
    }
    pub fn release(&mut self, lease: BufferLease) -> Result<(), GraphicsError> {
        if lease.generation != self.generation { return Err(GraphicsError::InvalidGeneration); }
        let (_, state) = self.entries.get_mut(&lease.id.0).ok_or(GraphicsError::UnknownBuffer)?;
        if *state != BufferState::InFlight(lease.lease_id) { return Err(GraphicsError::InvalidBufferState); }
        *state = BufferState::Available;
        Ok(())
    }
    /// Cancel a queued frame before it is submitted to the GPU.
    pub fn cancel_queued(&mut self, lease: BufferLease) -> Result<(), GraphicsError> {
        if lease.generation != self.generation { return Err(GraphicsError::InvalidGeneration); }
        let (_, state) = self.entries.get_mut(&lease.id.0).ok_or(GraphicsError::UnknownBuffer)?;
        if *state != BufferState::Queued(lease.lease_id) { return Err(GraphicsError::InvalidBufferState); }
        *state = BufferState::Available;
        Ok(())
    }
    pub fn unregister(&mut self, id: BufferId) -> Result<(), GraphicsError> {
        let (_, state) = self.entries.get(&id.0).ok_or(GraphicsError::UnknownBuffer)?;
        if *state != BufferState::Available { return Err(GraphicsError::BufferBusy); }
        self.entries.remove(&id.0);
        Ok(())
    }
    pub fn reset_generation(&mut self, generation: u64) -> Result<(), GraphicsError> {
        if generation <= self.generation { return Err(GraphicsError::InvalidGeneration); }
        self.entries.clear();
        self.generation = generation;
        Ok(())
    }
    pub fn len(&self) -> usize { self.entries.len() }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn layout() -> BufferLayout {
        BufferLayout { width: 1080, height: 2400, stride_bytes: 4320, format: PixelFormat::Rgba8888,
            offset_bytes: 0, allocation_bytes: 4320 * 2400 }
    }
    fn frame(sequence: u64) -> FrameDescriptor {
        FrameDescriptor { display_generation: 1, sequence, buffer_id: BufferId(sequence),
            acquire_fence: Some(FenceId(sequence)),
            damage: DamageRegion { rects: vec![Rect { left: 0, top: 0, right: 1080, bottom: 2400 }] },
            color_space: ColorSpace::Srgb, alpha_mode: AlphaMode::Premultiplied }
    }
    #[test]
    fn native_one_to_one_geometry_is_accepted() {
        let c = DisplayGeometry { guest_width: 1080, guest_height: 2400, output_width: 1080, output_height: 2400,
            density_dpi: 420, refresh_rate_millihz: 120_000, rotation: Rotation::R0, scale_mode: ScaleMode::OneToOne };
        assert_eq!(c.validate(), Ok(c));
    }
    #[test]
    fn one_to_one_rejects_implicit_resampling() {
        let c = DisplayGeometry { guest_width: 720, guest_height: 1600, output_width: 1080, output_height: 2400,
            density_dpi: 420, refresh_rate_millihz: 60_000, rotation: Rotation::R0, scale_mode: ScaleMode::OneToOne };
        assert_eq!(c.validate(), Err(GraphicsError::UnsupportedScale));
    }
    #[test]
    fn integer_nearest_requires_equal_integer_scale() {
        let c = DisplayGeometry { guest_width: 540, guest_height: 1200, output_width: 1080, output_height: 2400,
            density_dpi: 420, refresh_rate_millihz: 60_000, rotation: Rotation::R0, scale_mode: ScaleMode::IntegerNearest };
        assert!(c.validate().is_ok());
        assert_eq!(DisplayGeometry { output_height: 2300, ..c }.validate(), Err(GraphicsError::UnsupportedScale));
    }
    #[test]
    fn buffer_layout_checks_stride_and_allocation() {
        assert!(layout().validate().is_ok());
        assert_eq!(BufferLayout { stride_bytes: 4319, ..layout() }.validate(), Err(GraphicsError::InvalidStride));
        assert_eq!(BufferLayout { allocation_bytes: 100, ..layout() }.validate(), Err(GraphicsError::InvalidBufferRange));
    }
    #[test]
    fn damage_must_be_inside_buffer() {
        let d = DamageRegion { rects: vec![Rect { left: 0, top: 0, right: 1081, bottom: 10 }] };
        assert_eq!(d.validate(1080, 2400), Err(GraphicsError::InvalidDamage));
    }
    #[test]
    fn frame_queue_is_bounded_and_monotonic() {
        let mut q = FrameQueue::new(2, 1).unwrap();
        q.push(frame(1), layout()).unwrap(); q.push(frame(2), layout()).unwrap();
        assert_eq!(q.push(frame(3), layout()), Err(GraphicsError::QueueFull));
        assert_eq!(q.pop().unwrap().sequence, 1);
        assert_eq!(q.push(frame(2), layout()), Err(GraphicsError::InvalidSequence));
    }
    #[test]
    fn generation_reset_invalidates_queued_frames() {
        let mut q = FrameQueue::new(3, 1).unwrap(); q.push(frame(1), layout()).unwrap();
        q.reset_generation(2).unwrap(); assert!(q.is_empty());
        assert_eq!(q.push(frame(2), layout()), Err(GraphicsError::InvalidGeneration));
    }
    #[test]
    fn buffer_lease_has_checked_lifecycle() {
        let mut r = BufferRegistry::new(1, 4).unwrap();
        r.register(BufferRegistration { id: BufferId(7), layout: layout(),
            color_space: ColorSpace::Srgb, alpha_mode: AlphaMode::Opaque }).unwrap();
        let lease = r.acquire(BufferId(7)).unwrap();
        assert_eq!(r.acquire(BufferId(7)), Err(GraphicsError::BufferBusy));
        r.mark_in_flight(lease).unwrap();
        assert_eq!(r.unregister(BufferId(7)), Err(GraphicsError::BufferBusy));
        r.release(lease).unwrap(); r.unregister(BufferId(7)).unwrap();
        assert_eq!(r.len(), 0);
    }
    #[test]
    fn old_lease_cannot_release_a_reacquired_buffer() {
        let mut r = BufferRegistry::new(1, 4).unwrap();
        r.register(BufferRegistration { id: BufferId(11), layout: layout(),
            color_space: ColorSpace::Srgb, alpha_mode: AlphaMode::Opaque }).unwrap();
        let old = r.acquire(BufferId(11)).unwrap();
        r.mark_in_flight(old).unwrap();
        r.release(old).unwrap();
        let current = r.acquire(BufferId(11)).unwrap();
        r.mark_in_flight(current).unwrap();
        assert_ne!(old.lease_id, current.lease_id);
        assert_eq!(r.release(old), Err(GraphicsError::InvalidBufferState));
        assert_eq!(r.release(current), Ok(()));
    }
    #[test]
    fn queued_buffer_can_be_cancelled_but_not_released_as_completed() {
        let mut r = BufferRegistry::new(1, 4).unwrap();
        r.register(BufferRegistration { id: BufferId(12), layout: layout(),
            color_space: ColorSpace::Srgb, alpha_mode: AlphaMode::Opaque }).unwrap();
        let lease = r.acquire(BufferId(12)).unwrap();
        assert_eq!(r.release(lease), Err(GraphicsError::InvalidBufferState));
        r.cancel_queued(lease).unwrap();
        assert_eq!(r.acquire(BufferId(12)).unwrap().id, BufferId(12));
    }
    #[test]
    fn stale_lease_is_rejected_after_generation_change() {
        let mut r = BufferRegistry::new(1, 4).unwrap();
        r.register(BufferRegistration { id: BufferId(9), layout: layout(),
            color_space: ColorSpace::Srgb, alpha_mode: AlphaMode::Opaque }).unwrap();
        let lease = r.acquire(BufferId(9)).unwrap(); r.reset_generation(2).unwrap();
        assert_eq!(r.release(lease), Err(GraphicsError::InvalidGeneration));
    }
}
