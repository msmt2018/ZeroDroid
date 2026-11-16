#include "zerodroid/core/guest_memory_layout.hpp"

#include <cstdio>

using namespace zerodroid::core;

#define CHECK(value) do { if (!(value)) { std::fprintf(stderr, "CHECK failed at %s:%d: %s\n", __FILE__, __LINE__, #value); return 1; } } while (false)

int main() {
    constexpr std::uint64_t page = kGuestPageSize;
    const GuestMemoryRegion regions[] = {
        {0x40000000ull, 0x10000000ull, GuestMemoryRegionKind::Ram,
            GuestMemoryRead | GuestMemoryWrite | GuestMemoryExecute},
        {0x50000000ull, 0x1000ull, GuestMemoryRegionKind::Reserved, 0},
        {0x60000000ull, 0x2000ull, GuestMemoryRegionKind::Shared,
            GuestMemoryRead | GuestMemoryWrite},
    };

    GuestMemoryLayout layout;
    CHECK(layout.build(regions, 3, 0x10000000ull) == GuestMemoryLayoutStatus::Ok);
    CHECK(layout.region_count() == 3);
    CHECK(layout.ram_bytes() == 0x10000000ull);

    ResolvedGuestMemoryRange resolved{};
    CHECK(layout.resolve(0x40001000ull, page,
        GuestMemoryRead | GuestMemoryWrite, &resolved) == GuestMemoryLayoutStatus::Ok);
    CHECK(resolved.region_index == 0);
    CHECK(resolved.region_offset == page);
    CHECK(resolved.kind == GuestMemoryRegionKind::Ram);

    CHECK(layout.resolve(0x60000000ull, page,
        GuestMemoryRead | GuestMemoryWrite, &resolved) == GuestMemoryLayoutStatus::Ok);
    CHECK(layout.resolve(0x60000000ull, page,
        GuestMemoryExecute, &resolved) == GuestMemoryLayoutStatus::AccessDenied);
    CHECK(layout.resolve(0x50000000ull, page,
        GuestMemoryRead, &resolved) == GuestMemoryLayoutStatus::AccessDenied);

    CHECK(layout.resolve(0x4ffffff0ull, 0x20ull,
        GuestMemoryRead, &resolved) == GuestMemoryLayoutStatus::OutOfRange);
    CHECK(layout.resolve(UINT64_MAX - 7, 16,
        GuestMemoryRead, &resolved) == GuestMemoryLayoutStatus::AddressOverflow);
    CHECK(layout.resolve(0x40000000ull, 0,
        GuestMemoryRead, &resolved) == GuestMemoryLayoutStatus::InvalidArgument);

    const GuestMemoryRegion overlap[] = {
        {0x1000, 0x3000, GuestMemoryRegionKind::Ram, GuestMemoryRead | GuestMemoryWrite},
        {0x3000, 0x1000, GuestMemoryRegionKind::Shared, GuestMemoryRead | GuestMemoryWrite},
    };
    CHECK(layout.build(overlap, 2, 0x3000) == GuestMemoryLayoutStatus::OverlappingRegions);
    CHECK(layout.region_count() == 3); // Failed build does not replace the previous valid layout.

    const GuestMemoryRegion unaligned[] = {
        {0x1001, page, GuestMemoryRegionKind::Ram, GuestMemoryRead | GuestMemoryWrite},
    };
    CHECK(layout.build(unaligned, 1, page) == GuestMemoryLayoutStatus::UnalignedRegion);

    const GuestMemoryRegion overflow[] = {
        {UINT64_MAX - page + 1, page, GuestMemoryRegionKind::Ram, GuestMemoryRead | GuestMemoryWrite},
    };
    CHECK(layout.build(overflow, 1, page) == GuestMemoryLayoutStatus::AddressOverflow);

    const GuestMemoryRegion no_execute_shared[] = {
        {0x1000, page, GuestMemoryRegionKind::Shared,
            GuestMemoryRead | GuestMemoryWrite | GuestMemoryExecute},
    };
    CHECK(layout.build(no_execute_shared, 1, 0) == GuestMemoryLayoutStatus::InvalidArgument);
    return 0;
}
