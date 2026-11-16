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

#[derive(Debug, PartialEq, Eq)]
pub struct DownloadBuffer {
    expected_bytes: usize,
    bytes: Vec<u8>,
}

impl DownloadBuffer {
    fn new(expected_bytes: u64, max_bytes: u64) -> Result<Self, FastbootError> {
        if expected_bytes == 0 || expected_bytes > max_bytes {
            return Err(FastbootError::DownloadTooLarge);
        }
        let expected = usize::try_from(expected_bytes).map_err(|_| FastbootError::DownloadTooLarge)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(expected).map_err(|_| FastbootError::ResourceLimit)?;
        Ok(Self { expected_bytes: expected, bytes })
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<(), FastbootError> {
        let new_len = self.bytes.len().checked_add(chunk.len()).ok_or(FastbootError::DownloadOverflow)?;
        if new_len > self.expected_bytes {
            return Err(FastbootError::DownloadOverflow);
        }
        self.bytes.extend_from_slice(chunk);
        Ok(())
    }

    pub fn remaining_bytes(&self) -> usize {
        self.expected_bytes - self.bytes.len()
    }

    pub fn is_complete(&self) -> bool {
        self.bytes.len() == self.expected_bytes
    }

    fn into_bytes(self) -> Result<Vec<u8>, FastbootError> {
        if !self.is_complete() {
            return Err(FastbootError::DownloadIncomplete);
        }
        Ok(self.bytes)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum FastbootAction {
    Reply(Vec<u8>),
    DownloadReady { expected_bytes: u64 },
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
    pub image: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErasePlan {
    pub partition: String,
    pub capacity_bytes: u64,
}

pub struct FastbootSession {
    policy: FastbootPolicy,
    partitions: Vec<PartitionDescriptor>,
    download: Option<DownloadBuffer>,
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
        if partitions.len() > 4096 {
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
        Ok(Self { policy, partitions, download: None, active_slot })
    }

    pub fn accept_download_chunk(&mut self, chunk: &[u8]) -> Result<usize, FastbootError> {
        let download = self.download.as_mut().ok_or(FastbootError::DownloadNotActive)?;
        download.push(chunk)?;
        Ok(download.remaining_bytes())
    }

    pub fn handle(&mut self, input: &[u8]) -> Result<FastbootAction, FastbootError> {
        match parse_command(input)? {
            FastbootCommand::GetVar(name) => self.getvar(name),
            FastbootCommand::Download { size } => {
                self.download = Some(DownloadBuffer::new(size, self.policy.max_download_bytes)?);
                Ok(FastbootAction::DownloadReady { expected_bytes: size })
            }
            FastbootCommand::Flash { partition } => {
                let descriptor = self.partition(partition)?.clone();
                if !descriptor.flashable {
                    return Err(FastbootError::PartitionNotFlashable);
                }
                let download = self.download.take().ok_or(FastbootError::DownloadNotActive)?;
                let image = download.into_bytes()?;
                if u64::try_from(image.len()).map_err(|_| FastbootError::DownloadTooLarge)? > descriptor.capacity_bytes {
                    return Err(FastbootError::ImageExceedsPartition);
                }
                Ok(FastbootAction::Flash(FlashPlan {
                    partition: descriptor.name,
                    image,
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
            "is-userspace" => "yes".to_owned(),
            "max-download-size" => format!("{:08x}", self.policy.max_download_bytes.min(u64::from(u32::MAX))),
            "current-slot" => self.active_slot.map(|slot| slot.to_string()).unwrap_or_else(|| "no-slot".to_owned()),
            "all" => {
                let mut values = vec![
                    "version:0.4".to_owned(),
                    "is-userspace:yes".to_owned(),
                    format!("max-download-size:{:08x}", self.policy.max_download_bytes.min(u64::from(u32::MAX))),
                    format!("current-slot:{}", self.active_slot.map(|slot| slot.to_string()).unwrap_or_else(|| "no-slot".to_owned())),
                ];
                for partition in &self.partitions {
                    values.push(format!("partition-size:{}:{:x}", partition.name, partition.capacity_bytes));
                    values.push(format!("partition-type:{}:virtual", partition.name));
                }
                values.join("\n")
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
        assert_eq!(vm.handle(b"download:00000004"), Ok(FastbootAction::DownloadReady { expected_bytes: 4 }));
        assert_eq!(vm.accept_download_chunk(&[1, 2]), Ok(2));
        assert_eq!(vm.handle(b"flash:boot_a"), Err(FastbootError::DownloadIncomplete));
        assert_eq!(vm.accept_download_chunk(&[3, 4]), Ok(0));
        assert_eq!(vm.handle(b"flash:boot_a"), Ok(FastbootAction::Flash(FlashPlan {
            partition: "boot_a".to_owned(), image: vec![1, 2, 3, 4],
        })));
    }

    #[test]
    fn rejects_download_overflow_and_partition_overflow() {
        let mut vm = session();
        vm.handle(b"download:00000005").unwrap();
        assert_eq!(vm.accept_download_chunk(&[0; 6]), Err(FastbootError::DownloadOverflow));
        vm.handle(b"download:00000003").unwrap();
        vm.accept_download_chunk(&[1, 2, 3]).unwrap();
        assert_eq!(vm.handle(b"flash:boot_a"), Err(FastbootError::ImageExceedsPartition));
    }

    #[test]
    fn flash_and_erase_are_allowlisted_and_capability_gated() {
        let mut vm = session();
        vm.handle(b"download:00000001").unwrap();
        vm.accept_download_chunk(&[7]).unwrap();
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
