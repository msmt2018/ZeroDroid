//! Portable monotonic fence timeline contracts.
//!
//! This module does not wait on or signal OS/GPU fences. A platform backend
//! maps these points to native sync primitives and reports completion here.

use crate::GraphicsError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimelineId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FencePoint {
    pub timeline: TimelineId,
    pub value: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineSnapshot {
    pub timeline: TimelineId,
    pub last_issued: u64,
    pub last_signaled: u64,
}

/// Monotonic timeline used to describe ordered GPU/display completion.
#[derive(Debug, Clone)]
pub struct FenceTimeline {
    id: TimelineId,
    last_issued: u64,
    last_signaled: u64,
}

impl FenceTimeline {
    pub fn new(id: TimelineId) -> Result<Self, GraphicsError> {
        if id.0 == 0 { return Err(GraphicsError::InvalidFenceId); }
        Ok(Self { id, last_issued: 0, last_signaled: 0 })
    }

    /// Reserve a completion point before submitting work to a backend.
    pub fn issue(&mut self) -> Result<FencePoint, GraphicsError> {
        self.last_issued = self.last_issued.checked_add(1)
            .ok_or(GraphicsError::LeaseIdExhausted)?;
        Ok(FencePoint { timeline: self.id, value: self.last_issued })
    }

    /// Publish backend completion. Signaling a point also completes all lower points.
    pub fn signal(&mut self, point: FencePoint) -> Result<(), GraphicsError> {
        if point.timeline != self.id || point.value == 0 {
            return Err(GraphicsError::InvalidFenceId);
        }
        if point.value > self.last_issued {
            return Err(GraphicsError::InvalidSequence);
        }
        if point.value <= self.last_signaled {
            return Err(GraphicsError::InvalidSequence);
        }
        self.last_signaled = point.value;
        Ok(())
    }

    pub fn is_signaled(&self, point: FencePoint) -> Result<bool, GraphicsError> {
        if point.timeline != self.id || point.value == 0 {
            return Err(GraphicsError::InvalidFenceId);
        }
        if point.value > self.last_issued {
            return Err(GraphicsError::InvalidSequence);
        }
        Ok(point.value <= self.last_signaled)
    }

    pub fn snapshot(&self) -> TimelineSnapshot {
        TimelineSnapshot {
            timeline: self.id,
            last_issued: self.last_issued,
            last_signaled: self.last_signaled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issued_points_start_unsignaled_and_complete_monotonically() {
        let mut timeline = FenceTimeline::new(TimelineId(7)).unwrap();
        let first = timeline.issue().unwrap();
        let second = timeline.issue().unwrap();
        assert!(!timeline.is_signaled(first).unwrap());
        timeline.signal(second).unwrap();
        assert!(timeline.is_signaled(first).unwrap());
        assert!(timeline.is_signaled(second).unwrap());
        assert_eq!(timeline.snapshot().last_signaled, 2);
    }

    #[test]
    fn cannot_signal_a_point_that_was_not_issued() {
        let mut timeline = FenceTimeline::new(TimelineId(7)).unwrap();
        assert_eq!(
            timeline.signal(FencePoint { timeline: TimelineId(7), value: 1 }),
            Err(GraphicsError::InvalidSequence)
        );
    }

    #[test]
    fn rejects_foreign_and_duplicate_signals() {
        let mut timeline = FenceTimeline::new(TimelineId(7)).unwrap();
        let point = timeline.issue().unwrap();
        assert_eq!(
            timeline.signal(FencePoint { timeline: TimelineId(8), value: point.value }),
            Err(GraphicsError::InvalidFenceId)
        );
        timeline.signal(point).unwrap();
        assert_eq!(timeline.signal(point), Err(GraphicsError::InvalidSequence));
    }

    #[test]
    fn foreign_points_cannot_be_queried() {
        let timeline = FenceTimeline::new(TimelineId(7)).unwrap();
        assert_eq!(
            timeline.is_signaled(FencePoint { timeline: TimelineId(8), value: 1 }),
            Err(GraphicsError::InvalidFenceId)
        );
    }
}
