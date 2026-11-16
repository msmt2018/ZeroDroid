#include "zerodroid/core/guest_memory_layout.hpp"

#include <algorithm>
#include <limits>
#include <iterator>
#include <new>

namespace zerodroid::core {
namespace {
constexpr std::uint32_t kKnownPermissions =
    GuestMemoryRead | GuestMemoryWrite | GuestMemoryExecute;

bool valid_permissions(const GuestMemoryRegion& region) noexcept {
    if ((region.permissions & ~kKnownPermissions) != 0) return false;
    switch (region.kind) {
        case GuestMemoryRegionKind::Ram:
            return (region.permissions & GuestMemoryRead) != 0;
        case GuestMemoryRegionKind::Reserved:
            return region.permissions == 0;
        case GuestMemoryRegionKind::Shared:
            return (region.permissions & GuestMemoryRead) != 0 &&
                   (region.permissions & GuestMemoryExecute) == 0;
    }
    return false;
}
} // namespace

GuestMemoryLayoutStatus GuestMemoryLayout::build(
    const GuestMemoryRegion* regions,
    std::size_t region_count,
    std::uint64_t expected_ram_bytes) noexcept {
    if (regions == nullptr || region_count == 0 || expected_ram_bytes == 0) {
        return GuestMemoryLayoutStatus::InvalidArgument;
    }
    if (region_count > kGuestHardMaxMemoryRegions ||
        expected_ram_bytes > kGuestHardMaxMemoryBytes ||
        (expected_ram_bytes % kGuestPageSize) != 0) {
        return GuestMemoryLayoutStatus::ResourceLimit;
    }

    try {
        std::vector<GuestMemoryRegion> candidate;
        candidate.reserve(region_count);
        std::uint64_t total_ram = 0;

        for (std::size_t i = 0; i < region_count; ++i) {
            const auto& current = regions[i];
            if (current.size_bytes == 0) return GuestMemoryLayoutStatus::EmptyRegion;
            if ((current.guest_phys_addr % kGuestPageSize) != 0 ||
                (current.size_bytes % kGuestPageSize) != 0) {
                return GuestMemoryLayoutStatus::UnalignedRegion;
            }
            if (!valid_permissions(current)) {
                return GuestMemoryLayoutStatus::InvalidPermissions;
            }
            if (current.guest_phys_addr >
                std::numeric_limits<std::uint64_t>::max() - current.size_bytes) {
                return GuestMemoryLayoutStatus::AddressOverflow;
            }
            if (current.kind == GuestMemoryRegionKind::Ram) {
                if (total_ram > std::numeric_limits<std::uint64_t>::max() - current.size_bytes) {
                    return GuestMemoryLayoutStatus::AddressOverflow;
                }
                total_ram += current.size_bytes;
            }
            candidate.push_back(current);
        }

        std::sort(candidate.begin(), candidate.end(),
            [](const GuestMemoryRegion& left, const GuestMemoryRegion& right) {
                return left.guest_phys_addr < right.guest_phys_addr;
            });

        for (std::size_t i = 1; i < candidate.size(); ++i) {
            const auto& previous = candidate[i - 1];
            const auto previous_end = previous.guest_phys_addr + previous.size_bytes;
            if (candidate[i].guest_phys_addr < previous_end) {
                return GuestMemoryLayoutStatus::OverlappingRegions;
            }
        }
        if (total_ram != expected_ram_bytes) {
            return GuestMemoryLayoutStatus::RamSizeMismatch;
        }

        regions_.swap(candidate);
        ram_bytes_ = total_ram;
        return GuestMemoryLayoutStatus::Ok;
    } catch (const std::bad_alloc&) {
        return GuestMemoryLayoutStatus::ResourceLimit;
    } catch (...) {
        return GuestMemoryLayoutStatus::InvalidArgument;
    }
}

GuestMemoryLayoutStatus GuestMemoryLayout::resolve(
    std::uint64_t guest_phys_addr,
    std::uint64_t size_bytes,
    std::uint32_t requested_permissions,
    ResolvedGuestMemoryRange* out_range) const noexcept {
    if (out_range == nullptr || size_bytes == 0 ||
        requested_permissions == 0 ||
        (requested_permissions & ~kKnownPermissions) != 0) {
        return GuestMemoryLayoutStatus::InvalidArgument;
    }
    if (guest_phys_addr > std::numeric_limits<std::uint64_t>::max() - size_bytes) {
        return GuestMemoryLayoutStatus::AddressOverflow;
    }
    const auto requested_end = guest_phys_addr + size_bytes;

    const auto it = std::upper_bound(
        regions_.begin(), regions_.end(), guest_phys_addr,
        [](std::uint64_t address, const GuestMemoryRegion& region) {
            return address < region.guest_phys_addr;
        });
    if (it == regions_.begin()) return GuestMemoryLayoutStatus::OutOfRange;
    const auto selected = std::prev(it);
    const auto region_end = selected->guest_phys_addr + selected->size_bytes;
    if (guest_phys_addr < selected->guest_phys_addr || requested_end > region_end) {
        // A request must fit in one region. Callers split cross-region operations.
        return GuestMemoryLayoutStatus::OutOfRange;
    }
    if ((selected->permissions & requested_permissions) != requested_permissions) {
        return GuestMemoryLayoutStatus::AccessDenied;
    }

    out_range->region_index = static_cast<std::size_t>(std::distance(regions_.begin(), selected));
    out_range->region_offset = guest_phys_addr - selected->guest_phys_addr;
    out_range->guest_phys_addr = guest_phys_addr;
    out_range->size_bytes = size_bytes;
    out_range->kind = selected->kind;
    return GuestMemoryLayoutStatus::Ok;
}

std::size_t GuestMemoryLayout::region_count() const noexcept {
    return regions_.size();
}

std::uint64_t GuestMemoryLayout::ram_bytes() const noexcept {
    return ram_bytes_;
}

const GuestMemoryRegion* GuestMemoryLayout::region(std::size_t index) const noexcept {
    return index < regions_.size() ? &regions_[index] : nullptr;
}

} // namespace zerodroid::core
