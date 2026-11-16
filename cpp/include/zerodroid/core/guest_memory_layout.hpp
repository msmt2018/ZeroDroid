#ifndef ZERODROID_CORE_GUEST_MEMORY_LAYOUT_HPP
#define ZERODROID_CORE_GUEST_MEMORY_LAYOUT_HPP

#include <cstddef>
#include <cstdint>
#include <vector>

namespace zerodroid::core {

constexpr std::uint64_t kGuestPageSize = 4096;
constexpr std::uint64_t kGuestHardMaxMemoryBytes = 64ull * 1024ull * 1024ull * 1024ull;
constexpr std::size_t kGuestHardMaxMemoryRegions = 64;

enum class GuestMemoryRegionKind : std::uint32_t {
    Ram = 1,
    Reserved = 2,
    Shared = 3,
};

enum GuestMemoryPermission : std::uint32_t {
    GuestMemoryRead = 1u << 0,
    GuestMemoryWrite = 1u << 1,
    GuestMemoryExecute = 1u << 2,
};

enum class GuestMemoryLayoutStatus : std::uint32_t {
    Ok = 0,
    InvalidArgument,
    ResourceLimit,
    EmptyRegion,
    UnalignedRegion,
    AddressOverflow,
    OverlappingRegions,
    RamSizeMismatch,
    InvalidPermissions,
    OutOfRange,
    AccessDenied,
};

struct GuestMemoryRegion final {
    std::uint64_t guest_phys_addr{0};
    std::uint64_t size_bytes{0};
    GuestMemoryRegionKind kind{GuestMemoryRegionKind::Ram};
    std::uint32_t permissions{GuestMemoryRead | GuestMemoryWrite | GuestMemoryExecute};
};

struct ResolvedGuestMemoryRange final {
    std::size_t region_index{0};
    std::uint64_t region_offset{0};
    std::uint64_t guest_phys_addr{0};
    std::uint64_t size_bytes{0};
    GuestMemoryRegionKind kind{GuestMemoryRegionKind::Ram};
};

class GuestMemoryLayout final {
public:
    GuestMemoryLayout() = default;

    // Builds an immutable, bounded GPA layout. This does not allocate host RAM,
    // install Stage-2 mappings, pin pages, or create a KVM/AVF VM.
    [[nodiscard]] GuestMemoryLayoutStatus build(
        const GuestMemoryRegion* regions,
        std::size_t region_count,
        std::uint64_t expected_ram_bytes) noexcept;

    [[nodiscard]] GuestMemoryLayoutStatus resolve(
        std::uint64_t guest_phys_addr,
        std::uint64_t size_bytes,
        std::uint32_t requested_permissions,
        ResolvedGuestMemoryRange* out_range) const noexcept;

    [[nodiscard]] std::size_t region_count() const noexcept;
    [[nodiscard]] std::uint64_t ram_bytes() const noexcept;
    [[nodiscard]] const GuestMemoryRegion* region(std::size_t index) const noexcept;

private:
    std::vector<GuestMemoryRegion> regions_;
    std::uint64_t ram_bytes_{0};
};

} // namespace zerodroid::core

#endif
