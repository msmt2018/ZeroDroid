#ifndef ZERODROID_CORE_VM_BACKEND_HPP
#define ZERODROID_CORE_VM_BACKEND_HPP

#include <array>
#include <cstdint>

namespace zerodroid::core {

enum class BackendKind : std::uint32_t {
    Avf = 1,
    AuthorizedKvm = 2,
    TestOnly = 0xffff0001u,
};

enum class BackendAvailability : std::uint32_t {
    Unknown = 0,
    Available = 1,
    Unavailable = 2,
    PermissionDenied = 3,
    Incompatible = 4,
    Failed = 5,
};

enum class GuestArchitecture : std::uint32_t {
    AArch64 = 1,
};

enum class BackendVmState : std::uint32_t {
    Unknown = 0,
    Created = 1,
    Starting = 2,
    Running = 3,
    Stopping = 4,
    Stopped = 5,
    Failed = 6,
    Destroyed = 7,
};

enum class BackendStatus : std::uint32_t {
    Ok = 0,
    InvalidArgument = 1,
    Unsupported = 2,
    PermissionDenied = 3,
    ResourceExhausted = 4,
    InvalidState = 5,
    IoError = 6,
    Internal = 255,
};

struct BackendCapabilities final {
    BackendAvailability availability{BackendAvailability::Unknown};
    BackendKind kind{BackendKind::Avf};
    GuestArchitecture guest_architecture{GuestArchitecture::AArch64};
    bool hardware_virtualization{false};
    bool guest_memory_mapping{false};
    bool vcpu_execution{false};
    bool virtual_interrupt_controller{false};
    bool virtual_timer{false};
    bool guest_device_transport{false};
    std::uint32_t max_vcpus{0};
    std::uint64_t max_guest_memory_bytes{0};
};

struct VmConfig final {
    std::array<std::uint8_t, 16> vm_id{};
    std::uint64_t generation{0};
    GuestArchitecture guest_architecture{GuestArchitecture::AArch64};
    std::uint32_t vcpu_count{0};
    std::uint64_t guest_memory_bytes{0};
};

struct BackendError final {
    BackendStatus status{BackendStatus::Ok};
    std::int32_t native_code{0};
};

using BackendVmHandle = std::uint64_t;

class VmBackend {
public:
    virtual ~VmBackend() = default;

    [[nodiscard]] virtual BackendKind kind() const noexcept = 0;
    [[nodiscard]] virtual BackendCapabilities probe() noexcept = 0;

    virtual BackendStatus create(
        const VmConfig& config,
        BackendVmHandle* out_handle,
        BackendError* out_error) noexcept = 0;
    virtual BackendStatus start(
        BackendVmHandle handle,
        BackendError* out_error) noexcept = 0;
    virtual BackendStatus stop(
        BackendVmHandle handle,
        BackendError* out_error) noexcept = 0;
    virtual BackendStatus destroy(
        BackendVmHandle handle,
        BackendError* out_error) noexcept = 0;
    virtual BackendStatus query_state(
        BackendVmHandle handle,
        BackendVmState* out_state,
        BackendError* out_error) noexcept = 0;
};

} // namespace zerodroid::core

#endif
