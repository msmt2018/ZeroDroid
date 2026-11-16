#![forbid(unsafe_code)]

//! Platform-neutral input normalization for an Android-hosted ARM64 guest.
//!
//! Emits the eight-byte little-endian virtio_input_event wire representation.
//! It does not open /dev/uinput, create host input devices, access Android
//! hidden APIs, or claim that a VM transport is connected.

use core::fmt;

pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;
pub const SYN_REPORT: u16 = 0x00;
pub const ABS_MT_SLOT: u16 = 0x2f;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_TRACKING_ID: u16 = 0x39;
pub const ABS_MT_PRESSURE: u16 = 0x3a;
pub const BTN_TOUCH: u16 = 0x14a;
pub const BTN_TOOL_FINGER: u16 = 0x145;

pub const MAX_POINTERS: usize = 10;
pub const VIRTIO_INPUT_EVENT_SIZE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    InvalidDimensions, InvalidCoordinate, InvalidPressure, TooManyPointers,
    DuplicatePointer, UnknownPointer, IncompletePointerFrame, InvalidKeyCode,
    TrackingIdExhausted,
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{self:?}") }
}
impl std::error::Error for InputError {}

/// Host display dimensions and guest absolute-axis ranges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputConfig {
    pub host_width: u32,
    pub host_height: u32,
    pub guest_abs_max_x: u32,
    pub guest_abs_max_y: u32,
    pub guest_pressure_max: u32,
}

impl InputConfig {
    pub fn validate(self) -> Result<Self, InputError> {
        if self.host_width == 0 || self.host_height == 0
            || self.guest_abs_max_x == 0 || self.guest_abs_max_y == 0
            || self.guest_pressure_max == 0
            || self.guest_abs_max_x > i32::MAX as u32
            || self.guest_abs_max_y > i32::MAX as u32
            || self.guest_pressure_max > i32::MAX as u32 {
            return Err(InputError::InvalidDimensions);
        }
        Ok(self)
    }
    fn map_x(self, x: f32) -> Result<i32, InputError> {
        if !x.is_finite() { return Err(InputError::InvalidCoordinate); }
        Ok(((x / self.host_width as f32) * self.guest_abs_max_x as f32)
            .clamp(0.0, self.guest_abs_max_x as f32).round() as i32)
    }
    fn map_y(self, y: f32) -> Result<i32, InputError> {
        if !y.is_finite() { return Err(InputError::InvalidCoordinate); }
        Ok(((y / self.host_height as f32) * self.guest_abs_max_y as f32)
            .clamp(0.0, self.guest_abs_max_y as f32).round() as i32)
    }
    fn map_pressure(self, pressure: f32) -> Result<i32, InputError> {
        if !pressure.is_finite() { return Err(InputError::InvalidPressure); }
        Ok((pressure.clamp(0.0, 1.0) * self.guest_pressure_max as f32).round() as i32)
    }
}

/// Android MotionEvent pointer sample in display-local pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerSample {
    pub pointer_id: i32,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
}

/// ABI payload of Linux struct virtio_input_event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct VirtioInputEvent {
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
}
impl VirtioInputEvent {
    pub const fn new(event_type: u16, code: u16, value: i32) -> Self {
        Self { event_type, code, value }
    }
    pub fn encode_le(self) -> [u8; VIRTIO_INPUT_EVENT_SIZE] {
        let mut bytes = [0_u8; VIRTIO_INPUT_EVENT_SIZE];
        bytes[0..2].copy_from_slice(&self.event_type.to_le_bytes());
        bytes[2..4].copy_from_slice(&self.code.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.value.to_le_bytes());
        bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ActivePointer { host_id: i32, tracking_id: i32 }

#[derive(Debug, Clone)]
pub struct MultiTouchNormalizer {
    config: InputConfig,
    slots: [Option<ActivePointer>; MAX_POINTERS],
    next_tracking_id: i32,
}

impl MultiTouchNormalizer {
    pub fn new(config: InputConfig) -> Result<Self, InputError> {
        Ok(Self { config: config.validate()?, slots: [None; MAX_POINTERS], next_tracking_id: 1 })
    }
    pub fn active_pointer_count(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }
    pub fn pointer_down(&mut self, sample: PointerSample) -> Result<Vec<VirtioInputEvent>, InputError> {
        let x = self.config.map_x(sample.x)?;
        let y = self.config.map_y(sample.y)?;
        let pressure = self.config.map_pressure(sample.pressure)?;
        if self.find_slot(sample.pointer_id).is_some() { return Err(InputError::DuplicatePointer); }
        let slot = self.slots.iter().position(Option::is_none).ok_or(InputError::TooManyPointers)?;
        let tracking_id = self.next_tracking_id;
        let next = self.next_tracking_id.checked_add(1).ok_or(InputError::TrackingIdExhausted)?;
        let was_empty = self.active_pointer_count() == 0;
        self.next_tracking_id = next;
        self.slots[slot] = Some(ActivePointer { host_id: sample.pointer_id, tracking_id });

        let mut events = Vec::with_capacity(8);
        push_abs(&mut events, ABS_MT_SLOT, slot as i32);
        push_abs(&mut events, ABS_MT_TRACKING_ID, tracking_id);
        push_abs(&mut events, ABS_MT_POSITION_X, x);
        push_abs(&mut events, ABS_MT_POSITION_Y, y);
        push_abs(&mut events, ABS_MT_PRESSURE, pressure);
        if was_empty {
            push_key(&mut events, BTN_TOUCH, 1);
            push_key(&mut events, BTN_TOOL_FINGER, 1);
        }
        finish_frame(&mut events);
        Ok(events)
    }
    /// Updates all active pointers in one frame; every active pointer must occur exactly once.
    pub fn pointer_move(&mut self, samples: &[PointerSample]) -> Result<Vec<VirtioInputEvent>, InputError> {
        if samples.len() != self.active_pointer_count() { return Err(InputError::IncompletePointerFrame); }
        let mut seen = [false; MAX_POINTERS];
        let mut mapped = Vec::with_capacity(samples.len());
        for sample in samples {
            let slot = self.find_slot(sample.pointer_id).ok_or(InputError::UnknownPointer)?;
            if seen[slot] { return Err(InputError::DuplicatePointer); }
            seen[slot] = true;
            mapped.push((slot, self.config.map_x(sample.x)?, self.config.map_y(sample.y)?,
                self.config.map_pressure(sample.pressure)?));
        }
        if self.slots.iter().enumerate().any(|(i, slot)| slot.is_some() != seen[i]) {
            return Err(InputError::IncompletePointerFrame);
        }
        let mut events = Vec::with_capacity(mapped.len() * 5 + 1);
        for (slot, x, y, pressure) in mapped {
            push_abs(&mut events, ABS_MT_SLOT, slot as i32);
            push_abs(&mut events, ABS_MT_POSITION_X, x);
            push_abs(&mut events, ABS_MT_POSITION_Y, y);
            push_abs(&mut events, ABS_MT_PRESSURE, pressure);
        }
        finish_frame(&mut events);
        Ok(events)
    }
    pub fn pointer_up(&mut self, pointer_id: i32) -> Result<Vec<VirtioInputEvent>, InputError> {
        let slot = self.find_slot(pointer_id).ok_or(InputError::UnknownPointer)?;
        self.slots[slot] = None;
        let now_empty = self.active_pointer_count() == 0;
        let mut events = Vec::with_capacity(5);
        push_abs(&mut events, ABS_MT_SLOT, slot as i32);
        push_abs(&mut events, ABS_MT_TRACKING_ID, -1);
        if now_empty {
            push_key(&mut events, BTN_TOUCH, 0);
            push_key(&mut events, BTN_TOOL_FINGER, 0);
        }
        finish_frame(&mut events);
        Ok(events)
    }
    /// Cancels all contacts and releases touch buttons in a single frame.
    pub fn cancel_all(&mut self) -> Vec<VirtioInputEvent> {
        let mut events = Vec::with_capacity(self.active_pointer_count() * 2 + 3);
        for slot in 0..MAX_POINTERS {
            if self.slots[slot].take().is_some() {
                push_abs(&mut events, ABS_MT_SLOT, slot as i32);
                push_abs(&mut events, ABS_MT_TRACKING_ID, -1);
            }
        }
        if !events.is_empty() {
            push_key(&mut events, BTN_TOUCH, 0);
            push_key(&mut events, BTN_TOOL_FINGER, 0);
            finish_frame(&mut events);
        }
        events
    }
    fn find_slot(&self, host_id: i32) -> Option<usize> {
        self.slots.iter().position(|slot| slot.is_some_and(|p| p.host_id == host_id))
    }
}

/// Creates a key press/release frame. Android-to-Linux key mapping belongs in the Android adapter.
pub fn key_event(linux_key_code: u16, pressed: bool) -> Result<Vec<VirtioInputEvent>, InputError> {
    if linux_key_code == 0 { return Err(InputError::InvalidKeyCode); }
    let mut events = Vec::with_capacity(2);
    push_key(&mut events, linux_key_code, i32::from(pressed));
    finish_frame(&mut events);
    Ok(events)
}
fn push_abs(events: &mut Vec<VirtioInputEvent>, code: u16, value: i32) {
    events.push(VirtioInputEvent::new(EV_ABS, code, value));
}
fn push_key(events: &mut Vec<VirtioInputEvent>, code: u16, value: i32) {
    events.push(VirtioInputEvent::new(EV_KEY, code, value));
}
fn finish_frame(events: &mut Vec<VirtioInputEvent>) {
    events.push(VirtioInputEvent::new(EV_SYN, SYN_REPORT, 0));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn normalizer() -> MultiTouchNormalizer {
        MultiTouchNormalizer::new(InputConfig {
            host_width: 1000, host_height: 2000, guest_abs_max_x: 4095,
            guest_abs_max_y: 4095, guest_pressure_max: 255,
        }).unwrap()
    }
    #[test]
    fn maps_coordinates_and_emits_one_report_per_frame() {
        let mut input = normalizer();
        let events = input.pointer_down(PointerSample { pointer_id: 7, x: 500.0, y: 1000.0, pressure: 0.5 }).unwrap();
        assert_eq!(events[0], VirtioInputEvent::new(EV_ABS, ABS_MT_SLOT, 0));
        assert!(events.contains(&VirtioInputEvent::new(EV_ABS, ABS_MT_POSITION_X, 2048)));
        assert!(events.contains(&VirtioInputEvent::new(EV_ABS, ABS_MT_POSITION_Y, 2048)));
        assert_eq!(events.last(), Some(&VirtioInputEvent::new(EV_SYN, SYN_REPORT, 0)));
        assert_eq!(events.iter().filter(|e| e.event_type == EV_SYN).count(), 1);
    }
    #[test]
    fn maps_host_ids_to_stable_slots_and_updates_all_contacts() {
        let mut input = normalizer();
        let first = input.pointer_down(PointerSample { pointer_id: 9, x: 10.0, y: 20.0, pressure: 1.0 }).unwrap();
        let second = input.pointer_down(PointerSample { pointer_id: 2, x: 30.0, y: 40.0, pressure: 1.0 }).unwrap();
        assert_eq!(first[0].value, 0);
        assert_eq!(second[0].value, 1);
        let moved = input.pointer_move(&[
            PointerSample { pointer_id: 2, x: 300.0, y: 400.0, pressure: 0.8 },
            PointerSample { pointer_id: 9, x: 100.0, y: 200.0, pressure: 0.6 },
        ]).unwrap();
        assert!(moved.contains(&VirtioInputEvent::new(EV_ABS, ABS_MT_SLOT, 0)));
        assert!(moved.contains(&VirtioInputEvent::new(EV_ABS, ABS_MT_SLOT, 1)));
    }
    #[test]
    fn up_releases_one_contact_until_last_pointer() {
        let mut input = normalizer();
        input.pointer_down(PointerSample { pointer_id: 1, x: 1.0, y: 1.0, pressure: 1.0 }).unwrap();
        input.pointer_down(PointerSample { pointer_id: 3, x: 3.0, y: 3.0, pressure: 1.0 }).unwrap();
        let up = input.pointer_up(1).unwrap();
        assert!(!up.contains(&VirtioInputEvent::new(EV_KEY, BTN_TOUCH, 0)));
        let last = input.pointer_up(3).unwrap();
        assert!(last.contains(&VirtioInputEvent::new(EV_KEY, BTN_TOUCH, 0)));
        assert_eq!(input.active_pointer_count(), 0);
    }
    #[test]
    fn cancel_releases_every_contact() {
        let mut input = normalizer();
        input.pointer_down(PointerSample { pointer_id: 1, x: 1.0, y: 1.0, pressure: 1.0 }).unwrap();
        input.pointer_down(PointerSample { pointer_id: 2, x: 2.0, y: 2.0, pressure: 1.0 }).unwrap();
        let cancelled = input.cancel_all();
        assert_eq!(input.active_pointer_count(), 0);
        assert!(cancelled.contains(&VirtioInputEvent::new(EV_KEY, BTN_TOUCH, 0)));
        assert_eq!(cancelled.last(), Some(&VirtioInputEvent::new(EV_SYN, SYN_REPORT, 0)));
        assert!(input.cancel_all().is_empty());
    }
    #[test]
    fn invalid_move_frame_does_not_mutate_state() {
        let mut input = normalizer();
        input.pointer_down(PointerSample { pointer_id: 1, x: 1.0, y: 1.0, pressure: 1.0 }).unwrap();
        assert_eq!(input.pointer_move(&[]), Err(InputError::IncompletePointerFrame));
        assert_eq!(input.active_pointer_count(), 1);
    }
    #[test]
    fn rejects_non_finite_input_and_invalid_key_code() {
        let mut input = normalizer();
        assert_eq!(input.pointer_down(PointerSample { pointer_id: 1, x: f32::NAN, y: 0.0, pressure: 1.0 }),
            Err(InputError::InvalidCoordinate));
        assert_eq!(key_event(0, true), Err(InputError::InvalidKeyCode));
    }
    #[test]
    fn encodes_exact_little_endian_event_layout() {
        let event = VirtioInputEvent::new(0x1234, 0x5678, 0x1020_3040);
        assert_eq!(event.encode_le(), [0x34, 0x12, 0x78, 0x56, 0x40, 0x30, 0x20, 0x10]);
    }
    #[test]
    fn validates_dimensions_and_pointer_capacity() {
        assert_eq!(MultiTouchNormalizer::new(InputConfig {
            host_width: 0, host_height: 1, guest_abs_max_x: 1, guest_abs_max_y: 1,
            guest_pressure_max: 1,
        }).err(), Some(InputError::InvalidDimensions));
        let mut input = normalizer();
        for id in 0..MAX_POINTERS {
            input.pointer_down(PointerSample { pointer_id: id as i32, x: 0.0, y: 0.0, pressure: 1.0 }).unwrap();
        }
        assert_eq!(input.pointer_down(PointerSample { pointer_id: 99, x: 0.0, y: 0.0, pressure: 1.0 }),
            Err(InputError::TooManyPointers));
    }
}