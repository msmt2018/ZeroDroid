#include "zerodroid/core/vm.hpp"

#include <limits>

namespace zerodroid::core {
namespace {
constexpr std::uint32_t kHardMaxVcpus = 64;
constexpr std::uint64_t kHardMaxGuestMemoryBytes = 64ull * 1024ull * 1024ull * 1024ull;

bool nonzero_id(const std::array<std::uint8_t, 16>& id) noexcept {
    for (const auto byte : id) {
        if (byte != 0) return true;
    }
    return false;
}
} // namespace

Vm::Vm(VmConfig config, VmBackend& backend) noexcept
    : config_(config), backend_(backend) {}

bool Vm::valid_config() const noexcept {
    return nonzero_id(config_.vm_id) &&
           config_.generation != 0 &&
           config_.guest_architecture == GuestArchitecture::AArch64 &&
           config_.vcpu_count > 0 &&
           config_.vcpu_count <= kHardMaxVcpus &&
           config_.guest_memory_bytes >= 4096 &&
           config_.guest_memory_bytes <= kHardMaxGuestMemoryBytes &&
           (config_.guest_memory_bytes % 4096) == 0;
}

bool Vm::capabilities_satisfy_guest(
    const BackendCapabilities& capabilities) const noexcept {
    return capabilities.availability == BackendAvailability::Available &&
           capabilities.guest_architecture == config_.guest_architecture &&
           capabilities.hardware_virtualization &&
           capabilities.guest_memory_mapping &&
           capabilities.vcpu_execution &&
           capabilities.virtual_interrupt_controller &&
           capabilities.virtual_timer &&
           capabilities.guest_device_transport &&
           capabilities.max_vcpus >= config_.vcpu_count &&
           capabilities.max_guest_memory_bytes >= config_.guest_memory_bytes;
}

void Vm::transition_locked(VmLifecycleState next) noexcept {
    if (state_revision_ != std::numeric_limits<std::uint64_t>::max()) {
        ++state_revision_;
    }
    state_ = next;
}

void Vm::record_error_locked(
    BackendStatus status, const BackendError& error) noexcept {
    last_error_ = error;
    last_error_.status = status;
}

BackendStatus Vm::prepare() noexcept {
    std::lock_guard<std::mutex> lock(mutex_);
    if (state_ != VmLifecycleState::Created) return BackendStatus::InvalidState;
    transition_locked(VmLifecycleState::Preflight);

    if (!valid_config()) {
        record_error_locked(BackendStatus::InvalidArgument, {});
        transition_locked(VmLifecycleState::Failed);
        return BackendStatus::InvalidArgument;
    }

    const auto capabilities = backend_.probe();
    if (!capabilities_satisfy_guest(capabilities)) {
        const auto status = capabilities.availability == BackendAvailability::PermissionDenied
            ? BackendStatus::PermissionDenied
            : BackendStatus::Unsupported;
        record_error_locked(status, {});
        transition_locked(VmLifecycleState::Failed);
        return status;
    }

    BackendVmHandle handle = 0;
    BackendError error{};
    const auto result = backend_.create(config_, &handle, &error);
    if (result != BackendStatus::Ok || handle == 0) {
        const auto failure = result == BackendStatus::Ok
            ? BackendStatus::Internal : result;
        record_error_locked(failure, error);
        transition_locked(VmLifecycleState::Failed);
        return failure;
    }

    backend_handle_ = handle;
    last_error_ = {};
    transition_locked(VmLifecycleState::Prepared);
    return BackendStatus::Ok;
}

BackendStatus Vm::reconcile_backend_state_locked() noexcept {
    if (backend_handle_ == 0) return BackendStatus::InvalidState;
    BackendVmState backend_state = BackendVmState::Unknown;
    BackendError error{};
    const auto result = backend_.query_state(backend_handle_, &backend_state, &error);
    if (result != BackendStatus::Ok) {
        record_error_locked(result, error);
        transition_locked(VmLifecycleState::Failed);
        return result;
    }

    switch (backend_state) {
        case BackendVmState::Created:
            transition_locked(VmLifecycleState::Prepared);
            return BackendStatus::Ok;
        case BackendVmState::Starting:
        case BackendVmState::Running:
            transition_locked(VmLifecycleState::Running);
            return BackendStatus::Ok;
        case BackendVmState::Stopping:
        case BackendVmState::Stopped:
            transition_locked(VmLifecycleState::Stopped);
            return BackendStatus::Ok;
        case BackendVmState::Failed:
        case BackendVmState::Unknown:
        case BackendVmState::Destroyed:
            transition_locked(VmLifecycleState::Failed);
            return BackendStatus::InvalidState;
    }
    transition_locked(VmLifecycleState::Failed);
    return BackendStatus::Internal;
}

BackendStatus Vm::start() noexcept {
    std::lock_guard<std::mutex> lock(mutex_);
    if (state_ != VmLifecycleState::Prepared &&
        state_ != VmLifecycleState::Stopped) return BackendStatus::InvalidState;
    transition_locked(VmLifecycleState::Starting);
    BackendError error{};
    const auto result = backend_.start(backend_handle_, &error);
    if (result == BackendStatus::Ok) {
        last_error_ = {};
        transition_locked(VmLifecycleState::Running);
        return BackendStatus::Ok;
    }
    record_error_locked(result, error);
    (void)reconcile_backend_state_locked();
    return result;
}

BackendStatus Vm::stop() noexcept {
    std::lock_guard<std::mutex> lock(mutex_);
    if (state_ != VmLifecycleState::Running) return BackendStatus::InvalidState;
    transition_locked(VmLifecycleState::Stopping);
    BackendError error{};
    const auto result = backend_.stop(backend_handle_, &error);
    if (result == BackendStatus::Ok) {
        last_error_ = {};
        transition_locked(VmLifecycleState::Stopped);
        return BackendStatus::Ok;
    }
    record_error_locked(result, error);
    (void)reconcile_backend_state_locked();
    return result;
}

BackendStatus Vm::destroy() noexcept {
    std::lock_guard<std::mutex> lock(mutex_);
    if (state_ == VmLifecycleState::Destroyed ||
        state_ == VmLifecycleState::Destroying) return BackendStatus::InvalidState;

    if (state_ == VmLifecycleState::Running) {
        transition_locked(VmLifecycleState::Stopping);
        BackendError stop_error{};
        const auto stopped = backend_.stop(backend_handle_, &stop_error);
        if (stopped != BackendStatus::Ok) {
            record_error_locked(stopped, stop_error);
            (void)reconcile_backend_state_locked();
            return stopped;
        }
        transition_locked(VmLifecycleState::Stopped);
    }

    transition_locked(VmLifecycleState::Destroying);
    if (backend_handle_ != 0) {
        BackendError error{};
        const auto result = backend_.destroy(backend_handle_, &error);
        if (result != BackendStatus::Ok) {
            record_error_locked(result, error);
            transition_locked(VmLifecycleState::Failed);
            return result;
        }
    }
    backend_handle_ = 0;
    last_error_ = {};
    transition_locked(VmLifecycleState::Destroyed);
    return BackendStatus::Ok;
}

VmSnapshot Vm::snapshot() const noexcept {
    std::lock_guard<std::mutex> lock(mutex_);
    return VmSnapshot{state_, state_revision_, backend_.kind(), backend_handle_, last_error_};
}

} // namespace zerodroid::core
