#ifndef ZERODROID_CORE_VM_HPP
#define ZERODROID_CORE_VM_HPP

#include "zerodroid/core/vm_backend.hpp"

#include <cstdint>
#include <mutex>

namespace zerodroid::core {

enum class VmLifecycleState : std::uint32_t {
    Created = 0,
    Preflight = 1,
    Prepared = 2,
    Starting = 3,
    Running = 4,
    Stopping = 5,
    Stopped = 6,
    Failed = 7,
    Destroying = 8,
    Destroyed = 9,
};

struct VmSnapshot final {
    VmLifecycleState state{VmLifecycleState::Created};
    std::uint64_t state_revision{0};
    BackendKind backend_kind{BackendKind::Avf};
    BackendVmHandle backend_handle{0};
    BackendError last_error{};
};

class Vm final {
public:
    Vm(VmConfig config, VmBackend& backend) noexcept;
    Vm(const Vm&) = delete;
    Vm& operator=(const Vm&) = delete;

    [[nodiscard]] BackendStatus prepare() noexcept;
    [[nodiscard]] BackendStatus start() noexcept;
    [[nodiscard]] BackendStatus stop() noexcept;
    [[nodiscard]] BackendStatus destroy() noexcept;
    [[nodiscard]] VmSnapshot snapshot() const noexcept;

private:
    [[nodiscard]] bool valid_config() const noexcept;
    [[nodiscard]] bool capabilities_satisfy_guest(
        const BackendCapabilities& capabilities) const noexcept;
    void transition_locked(VmLifecycleState next) noexcept;
    void record_error_locked(BackendStatus status, const BackendError& error) noexcept;
    [[nodiscard]] BackendStatus reconcile_backend_state_locked() noexcept;

    VmConfig config_;
    VmBackend& backend_;
    mutable std::mutex mutex_;
    VmLifecycleState state_{VmLifecycleState::Created};
    std::uint64_t state_revision_{0};
    BackendVmHandle backend_handle_{0};
    BackendError last_error_{};
};

} // namespace zerodroid::core

#endif
