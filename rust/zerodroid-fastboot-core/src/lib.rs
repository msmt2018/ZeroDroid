#![forbid(unsafe_code)]

use core::fmt;

pub const MAX_COMMAND_BYTES: usize = 4096;
pub const MAX_PARTITION_NAME_BYTES: usize = 64;
pub const DEFAULT_MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastbootError {
    EmptyCommand,
    CommandTooLarge,
    NonAsciiCommand,
    MalformedCommand,
    UnknownCommand,
    InvalidNumber,
    DownloadTooLarge,
    DownloadNotActive,
    DownloadAlreadyActive,
    DownloadIdMismatch,
    DownloadIncomplete,
    DownloadOverflow,
    UnknownPartition,
    PartitionNotFlashable,
    PartitionNotErasable,
    ImageExceedsPartition,
    InvalidPartitionName,
    InvalidSlot,
    SlotSwitchingDisabled,
    ResourceLimit,
    ResponseTooLarge,
}

impl fmt::Display for FastbootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FastbootCommand<'a> {
    GetVar(&'a str),
    Download { size: u64 },
    Flash { partition: &'a str },
    Erase { partition: &'a str },
    SetActive { slot: char },
    Reboot,
    RebootBootloader,
    Continue,
}

pub fn parse_command(input: &[u8]) -> Result<FastbootCommand<'_>, FastbootError> {
    if input.is_empty() {
        return Err(FastbootError::EmptyCommand);
    }
    if input.len() > MAX_COMMAND_BYTES {
        return Err(FastbootError::CommandTooLarge);
    }
    if !input.is_ascii() || input.iter().any(|b| *b == 0 || *b == b'\n' || *b == b'\r') {
        return Err(FastbootError::NonAsciiCommand);
    }
    let command = core::str::from_utf8(input).map_err(|_| FastbootError::NonAsciiCommand)?;
    if let Some(value) = command.strip_prefix("getvar:") {
        if value.is_empty() {
            return Err(FastbootError::MalformedCommand);
        }
        return Ok(FastbootCommand::GetVar(value));
    }
    if let Some(value) = command.strip_prefix("download:") {
        if value.len() != 8 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(FastbootError::MalformedCommand);
        }
        let size = u64::from(u32::from_str_radix(value, 16).map_err(|_| FastbootError::InvalidNumber)?);
        return Ok(FastbootCommand::Download { size });
    }
    if let Some(partition) = command.strip_prefix("flash:") {
        validate_partition_name(partition)?;
        return Ok(FastbootCommand::Flash { partition });
    }
    if let Some(partition) = command.strip_prefix("erase:") {
        validate_partition_name(partition)?;
        return Ok(FastbootCommand::Erase { partition });
    }
    if let Some(slot) = command.strip_prefix("set_active:") {
        if slot.len() != 1 {
            return Err(FastbootError::InvalidSlot);
        }
        let slot = slot.chars().next().ok_or(FastbootError::InvalidSlot)?;
        if slot != 'a' && slot != 'b' {
            return Err(FastbootError::InvalidSlot);
        }
        return Ok(FastbootCommand::SetActive { slot });
    }
    match command {
        "reboot" => Ok(FastbootCommand::Reboot),
        "reboot-bootloader" => Ok(FastbootCommand::RebootBootloader),
        "continue" => Ok(FastbootCommand::Continue),
        _ => Err(FastbootError::UnknownCommand),
    }
}

fn validate_partition_name(name: &str) -> Result<(), FastbootError> {
    if name.is_empty()
        || name.len() > MAX_PARTITION_NAME_BYTES
        || !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-' || b == b'.')
        || name == "."
        || name == ".."
    {
        return Err(FastbootError::InvalidPartitionName);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionDescriptor {
    pub name: String,
    pub capacity_bytes: u64,
    pub flashable: bool,
    pub erasable: bool,
}

impl PartitionDescriptor {
    pub fn new(
        name: impl Into<String>,
        capacity_bytes: u64,
        flashable: bool,
        erasable: bool,
    ) -> Result<Self, FastbootError> {
        let name = name.into();
        validate_partition_name(&name)?;
        if capacity_bytes == 0 {
            return Err(FastbootError::ResourceLimit);
        }
        Ok(Self { name, capacity_bytes, flashable, erasable })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FastbootPolicy {
    pub max_download_bytes: u64,
    pub slot_switching_enabled: bool,
}

impl Default for FastbootPolicy {
    fn default() -> Self {
        Self {
            max_download_bytes: DEFAULT_MAX_DOWNLOAD_BYTES,
            slot_switching_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DownloadProgress {
    id: u64,
    expected_bytes: u64,
    received_bytes: u64,
}

/// A write reservation. The transport writes these bytes to a bounded per-VM
/// staging object first, then confirms the reservation with the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadWrite {
    download_id: u64,
    offset: u64,
    length: u64,
}

impl DownloadWrite {
    pub fn download_id(self) -> u64 { self.download_id }
    pub fn offset(self) -> u64 { self.offset }
    pub fn length(self) -> u64 { self.length }
}

#[derive(Debug, PartialEq, Eq)]
pub enum FastbootAction {
    Reply(Vec<u8>),
    DownloadReady { download_id: u64, expected_bytes: u64 },
    Flash(FlashPlan),
    Erase(ErasePlan),
    SetActiveSlot(char),
    Reboot,
    RebootBootloader,
    Continue,
}

#[derive(Debug, PartialEq, Eq)]
pub struct FlashPlan {
    pub partition: String,
    pub download_id: u64,
    pub image_size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErasePlan {
    pub partition: String,
    pub capacity_bytes: u64,
}

pub struct FastbootSession {
    policy: FastbootPolicy,
    partitions: Vec<PartitionDescriptor>,
    download: Option<DownloadProgress>,
    next_download_id: u64,
    active_slot: Option<char>,
}

impl FastbootSession {
    pub fn new(
        policy: FastbootPolicy,
        partitions: Vec<PartitionDescriptor>,
        active_slot: Option<char>,
    ) -> Result<Self, FastbootError> {
        if active_slot.is_some_and(|slot| slot != 'a' && slot != 'b') {
            return Err(FastbootError::InvalidSlot);
        }
        if policy.max_download_bytes == 0 {
            return Err(FastbootError::DownloadTooLarge);
        }
        if partitions.len() > 256 {
            return Err(FastbootError::ResourceLimit);
        }
        for (index, partition) in partitions.iter().enumerate() {
            validate_partition_name(&partition.name)?;
            if partition.capacity_bytes == 0
                || partitions[..index].iter().any(|previous| previous.name == partition.name)
            {
                return Err(FastbootError::ResourceLimit);
            }
        }
        Ok(Self { policy, partitions, download: None, next_download_id: 1, active_slot })
    }

    /// Reserve the next sequential write. The transport must persist the chunk
    /// to a private per-VM staging object before calling confirm_download_write.
    pub fn prepare_download_write(&self, chunk_len: usize) -> Result<DownloadWrite, FastbootError> {
        let download = self.download.as_ref().ok_or(FastbootError::DownloadNotActive)?;
        let length = u64::try_from(chunk_len).map_err(|_| FastbootError::DownloadOverflow)?;
        let end = download.received_bytes.checked_add(length).ok_or(FastbootError::DownloadOverflow)?;
        if length == 0 || end > download.expected_bytes {
            return Err(FastbootError::DownloadOverflow);
        }
        Ok(DownloadWrite {
            download_id: download.id,
            offset: download.received_bytes,
            length,
        })
    }

    /// Commit a write reservation only after the staging backend confirms that
    /// the corresponding bytes were durably accepted.
    pub fn confirm_download_write(&mut self, write: DownloadWrite) -> Result<u64, FastbootError> {
        let download = self.download.as_mut().ok_or(FastbootError::DownloadNotActive)?;
        if write.download_id != download.id || write.offset != download.received_bytes {
            return Err(FastbootError::DownloadOverflow);
        }
        let end = write.offset.checked_add(write.length).ok_or(FastbootError::DownloadOverflow)?;
        if write.length == 0 || end > download.expected_bytes {
            return Err(FastbootError::DownloadOverflow);
        }
        download.received_bytes = end;
        Ok(download.expected_bytes - download.received_bytes)
    }

    /// Abort an incomplete download after transport failure, timeout, or user
    /// cancellation. The caller must also delete the matching private staging
    /// object; an unrelated or stale token is never allowed to cancel a session.
    pub fn cancel_download(&mut self, download_id: u64) -> Result<(), FastbootError> {
        let active = self.download.as_ref().ok_or(FastbootError::DownloadNotActive)?;
        if active.id != download_id {
            return Err(FastbootError::DownloadIdMismatch);
        }
        self.download = None;
        Ok(())
    }

    pub fn handle(&mut self, input: &[u8]) -> Result<FastbootAction, FastbootError> {
        match parse_command(input)? {
            FastbootCommand::GetVar(name) => self.getvar(name),
            FastbootCommand::Download { size } => {
                if self.download.is_some() {
                    return Err(FastbootError::DownloadAlreadyActive);
                }
                if size == 0 || size > self.policy.max_download_bytes {
                    return Err(FastbootError::DownloadTooLarge);
                }
                let download_id = self.next_download_id;
                self.next_download_id = self.next_download_id.checked_add(1).ok_or(FastbootError::ResourceLimit)?;
                self.download = Some(DownloadProgress { id: download_id, expected_bytes: size, received_bytes: 0 });
                Ok(FastbootAction::DownloadReady { download_id, expected_bytes: size })
            }
            FastbootCommand::Flash { partition } => {
                let descriptor = self.partition(partition)?.clone();
                if !descriptor.flashable {
                    return Err(FastbootError::PartitionNotFlashable);
                }
                let download = self.download.as_ref().ok_or(FastbootError::DownloadNotActive)?;
                if download.received_bytes != download.expected_bytes {
                    return Err(FastbootError::DownloadIncomplete);
                }
                if download.expected_bytes > descriptor.capacity_bytes {
                    return Err(FastbootError::ImageExceedsPartition);
                }
                let download = self.download.take().ok_or(FastbootError::DownloadNotActive)?;
                Ok(FastbootAction::Flash(FlashPlan {
                    partition: descriptor.name,
                    download_id: download.id,
                    image_size_bytes: download.expected_bytes,
                }))
            }
            FastbootCommand::Erase { partition } => {
                let descriptor = self.partition(partition)?;
                if !descriptor.erasable {
                    return Err(FastbootError::PartitionNotErasable);
                }
                Ok(FastbootAction::Erase(ErasePlan {
                    partition: descriptor.name.clone(),
                    capacity_bytes: descriptor.capacity_bytes,
                }))
            }
            FastbootCommand::SetActive { slot } => {
                if !self.policy.slot_switching_enabled {
                    return Err(FastbootError::SlotSwitchingDisabled);
                }
                self.active_slot = Some(slot);
                Ok(FastbootAction::SetActiveSlot(slot))
            }
            FastbootCommand::Reboot => Ok(FastbootAction::Reboot),
            FastbootCommand::RebootBootloader => Ok(FastbootAction::RebootBootloader),
            FastbootCommand::Continue => Ok(FastbootAction::Continue),
        }
    }

    pub fn active_slot(&self) -> Option<char> {
        self.active_slot
    }

    fn partition(&self, name: &str) -> Result<&PartitionDescriptor, FastbootError> {
        self.partitions.iter().find(|partition| partition.name == name)
            .ok_or(FastbootError::UnknownPartition)
    }

    fn getvar(&self, name: &str) -> Result<FastbootAction, FastbootError> {
        let value = match name {
            "version" => "0.4".to_owned(),
            "is-userspace" => "no".to_owned(),
            "max-download-size" => format!("{:08x}", self.policy.max_download_bytes.min(u64::from(u32::MAX))),
            "current-slot" => self.active_slot.map(|slot| slot.to_string()).unwrap_or_else(|| "no-slot".to_owned()),
            "all" => {
                let mut values = vec![
                    "version:0.4".to_owned(),
                    "is-userspace:no".to_owned(),
                    format!("max-download-size:{:08x}", self.policy.max_download_bytes.min(u64::from(u32::MAX))),
                    format!("current-slot:{}", self.active_slot.map(|slot| slot.to_string()).unwrap_or_else(|| "no-slot".to_owned())),
                ];
                for partition in &self.partitions {
                    values.push(format!("partition-size:{}:{:x}", partition.name, partition.capacity_bytes));
                    values.push(format!("partition-type:{}:virtual", partition.name));
                }
                let response = values.join("\n");
                if response.len() > 64 * 1024 {
                    return Err(FastbootError::ResponseTooLarge);
                }
                response
            }
            _ if name.starts_with("partition-size:") => {
                let partition_name = &name["partition-size:".len()..];
                validate_partition_name(partition_name)?;
                format!("{:x}", self.partition(partition_name)?.capacity_bytes)
            }
            _ if name.starts_with("partition-type:") => {
                let partition_name = &name["partition-type:".len()..];
                validate_partition_name(partition_name)?;
                self.partition(partition_name)?;
                "virtual".to_owned()
            }
            _ => return Err(FastbootError::UnknownCommand),
        };
        Ok(FastbootAction::Reply(value.into_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> FastbootSession {
        FastbootSession::new(
            FastbootPolicy { max_download_bytes: 1024, slot_switching_enabled: true },
            vec![
                PartitionDescriptor::new("boot_a", 512, true, true).unwrap(),
                PartitionDescriptor::new("system_a", 4096, true, false).unwrap(),
            ],
            Some('a'),
        ).unwrap()
    }

    #[test]
    fn parses_supported_commands_and_rejects_unknown() {
        assert_eq!(parse_command(b"download:00000010"), Ok(FastbootCommand::Download { size: 16 }));
        assert_eq!(parse_command(b"flash:boot_a"), Ok(FastbootCommand::Flash { partition: "boot_a" }));
        assert_eq!(parse_command(b"set_active:b"), Ok(FastbootCommand::SetActive { slot: 'b' }));
        assert_eq!(parse_command(b"reboot-bootloader"), Ok(FastbootCommand::RebootBootloader));
        assert_eq!(parse_command(b"oem unlock"), Err(FastbootError::UnknownCommand));
    }

    #[test]
    fn rejects_malformed_and_path_like_partition_names() {
        assert_eq!(parse_command(b"download:xyz"), Err(FastbootError::MalformedCommand));
        assert_eq!(parse_command(b"flash:../boot"), Err(FastbootError::InvalidPartitionName));
        assert_eq!(parse_command(b"flash:/dev/block/by-name/boot"), Err(FastbootError::InvalidPartitionName));
        assert_eq!(parse_command(b"flash:BOOT"), Err(FastbootError::InvalidPartitionName));
        assert_eq!(parse_command(b"flash:boot\0"), Err(FastbootError::NonAsciiCommand));
    }

    #[test]
    fn download_is_bounded_and_requires_exact_length() {
        let mut vm = session();
        let ready = vm.handle(b"download:00000004").unwrap();
        let download_id = match ready {
            FastbootAction::DownloadReady { download_id, expected_bytes: 4 } => download_id,
            _ => panic!("unexpected action"),
        };
        let first = vm.prepare_download_write(2).unwrap();
        assert_eq!(first.download_id(), download_id);
        assert_eq!(first.offset(), 0);
        assert_eq!(vm.confirm_download_write(first), Ok(2));
        assert_eq!(vm.handle(b"flash:boot_a"), Err(FastbootError::DownloadIncomplete));
        let second = vm.prepare_download_write(2).unwrap();
        assert_eq!(second.offset(), 2);
        assert_eq!(vm.confirm_download_write(second), Ok(0));
        assert_eq!(vm.handle(b"flash:boot_a"), Ok(FastbootAction::Flash(FlashPlan {
            partition: "boot_a".to_owned(), download_id, image_size_bytes: 4,
        })));
    }

    #[test]
    fn rejects_overlapping_download_sessions_and_replayed_write_confirmations() {
        let mut vm = session();
        vm.handle(b"download:00000002").unwrap();
        assert_eq!(vm.handle(b"download:00000002"), Err(FastbootError::DownloadAlreadyActive));
        let write = vm.prepare_download_write(1).unwrap();
        assert_eq!(vm.confirm_download_write(write), Ok(1));
        assert_eq!(vm.confirm_download_write(write), Err(FastbootError::DownloadOverflow));
    }

    #[test]
    fn download_cancellation_requires_the_matching_session_token() {
        let mut vm = session();
        let id = match vm.handle(b"download:00000002").unwrap() {
            FastbootAction::DownloadReady { download_id, .. } => download_id,
            _ => panic!("unexpected action"),
        };
        assert_eq!(vm.cancel_download(id + 1), Err(FastbootError::DownloadIdMismatch));
        assert_eq!(vm.cancel_download(id), Ok(()));
        assert_eq!(vm.cancel_download(id), Err(FastbootError::DownloadNotActive));
    }

    #[test]
    fn rejects_download_overflow_and_partition_overflow() {
        let mut vm = session();
        let first = vm.handle(b"download:00000005").unwrap();
        let first_id = match first {
            FastbootAction::DownloadReady { download_id, .. } => download_id,
            _ => panic!("unexpected action"),
        };
        assert_eq!(vm.prepare_download_write(6), Err(FastbootError::DownloadOverflow));
        assert_eq!(vm.cancel_download(first_id + 1), Err(FastbootError::DownloadIdMismatch));
        vm.cancel_download(first_id).unwrap();
        vm.handle(b"download:00000201").unwrap();
        let write = vm.prepare_download_write(513).unwrap();
        vm.confirm_download_write(write).unwrap();
        assert_eq!(vm.handle(b"flash:boot_a"), Err(FastbootError::ImageExceedsPartition));
    }

    #[test]
    fn flash_and_erase_are_allowlisted_and_capability_gated() {
        let mut vm = session();
        vm.handle(b"download:00000001").unwrap();
        let write = vm.prepare_download_write(1).unwrap();
        vm.confirm_download_write(write).unwrap();
        assert_eq!(vm.handle(b"flash:userdata"), Err(FastbootError::UnknownPartition));
        assert_eq!(vm.handle(b"erase:system_a"), Err(FastbootError::PartitionNotErasable));
        assert!(matches!(vm.handle(b"erase:boot_a"), Ok(FastbootAction::Erase(_))));
    }

    #[test]
    fn slot_switching_is_explicit() {
        let mut vm = session();
        assert_eq!(vm.handle(b"set_active:b"), Ok(FastbootAction::SetActiveSlot('b')));
        assert_eq!(vm.active_slot(), Some('b'));
        let mut disabled = FastbootSession::new(
            FastbootPolicy { max_download_bytes: 1024, slot_switching_enabled: false },
            vec![PartitionDescriptor::new("boot_a", 512, true, true).unwrap()],
            None,
        ).unwrap();
        assert_eq!(disabled.handle(b"set_active:a"), Err(FastbootError::SlotSwitchingDisabled));
    }

    #[test]
    fn reports_only_virtual_partition_metadata() {
        let mut vm = session();
        assert_eq!(vm.handle(b"getvar:partition-size:boot_a"), Ok(FastbootAction::Reply(b"200".to_vec())));
        assert_eq!(vm.handle(b"getvar:partition-type:boot_a"), Ok(FastbootAction::Reply(b"virtual".to_vec())));
        assert_eq!(vm.handle(b"getvar:current-slot"), Ok(FastbootAction::Reply(b"a".to_vec())));
    }
}
