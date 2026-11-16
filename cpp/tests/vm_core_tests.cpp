#include "zerodroid/core/vm.hpp"

#include <cstdio>

using namespace zerodroid::core;

#define CHECK(value) do { if (!(value)) { std::fprintf(stderr, "CHECK failed at %s:%d: %s\n", __FILE__, __LINE__, #value); return 1; } } while (false)

class TestBackend final : public VmBackend {
public:
    BackendCapabilities capabilities{
        BackendAvailability::Available, BackendKind::TestOnly,
        GuestArchitecture::AArch64, true, true, true, true, true, true,
        8, 8ull * 1024ull * 1024ull * 1024ull
    };
    BackendVmState state{BackendVmState::Created};
    BackendStatus fail_start{BackendStatus::Ok};
    BackendVmHandle next_handle{1};
    int create_calls{0};
    int destroy_calls{0};

    BackendKind kind() const noexcept override { return BackendKind::TestOnly; }
    BackendCapabilities probe() noexcept override { return capabilities; }
    BackendStatus create(const VmConfig&, BackendVmHandle* out, BackendError*) noexcept override {
        ++create_calls;
        *out = next_handle++;
        state = BackendVmState::Created;
        return BackendStatus::Ok;
    }
    BackendStatus start(BackendVmHandle, BackendError*) noexcept override {
        if (fail_start != BackendStatus::Ok) return fail_start;
        state = BackendVmState::Running;
        return BackendStatus::Ok;
    }
    BackendStatus stop(BackendVmHandle, BackendError*) noexcept override {
        state = BackendVmState::Stopped;
        return BackendStatus::Ok;
    }
    BackendStatus destroy(BackendVmHandle, BackendError*) noexcept override {
        ++destroy_calls;
        state = BackendVmState::Destroyed;
        return BackendStatus::Ok;
    }
    BackendStatus query_state(BackendVmHandle, BackendVmState* out, BackendError*) noexcept override {
        *out = state;
        return BackendStatus::Ok;
    }
};

VmConfig valid_config() {
    VmConfig config{};
    config.vm_id[0] = 0x5a;
    config.generation = 1;
    config.guest_architecture = GuestArchitecture::AArch64;
    config.vcpu_count = 4;
    config.guest_memory_bytes = 1024ull * 1024ull * 1024ull;
    return config;
}

int main() {
    TestBackend backend;
    Vm vm(valid_config(), backend);
    CHECK(vm.snapshot().state == VmLifecycleState::Created);
    CHECK(vm.prepare() == BackendStatus::Ok);
    CHECK(vm.snapshot().state == VmLifecycleState::Prepared);
    CHECK(backend.create_calls == 1);
    CHECK(vm.start() == BackendStatus::Ok);
    CHECK(vm.snapshot().state == VmLifecycleState::Running);
    CHECK(vm.stop() == BackendStatus::Ok);
    CHECK(vm.snapshot().state == VmLifecycleState::Stopped);
    CHECK(vm.destroy() == BackendStatus::Ok);
    CHECK(vm.snapshot().state == VmLifecycleState::Destroyed);
    CHECK(backend.destroy_calls == 1);
    CHECK(vm.destroy() == BackendStatus::InvalidState);

    TestBackend denied;
    denied.capabilities.availability = BackendAvailability::PermissionDenied;
    Vm denied_vm(valid_config(), denied);
    CHECK(denied_vm.prepare() == BackendStatus::PermissionDenied);
    CHECK(denied.create_calls == 0);
    CHECK(denied_vm.snapshot().state == VmLifecycleState::Failed);

    TestBackend failed_start;
    Vm start_vm(valid_config(), failed_start);
    CHECK(start_vm.prepare() == BackendStatus::Ok);
    failed_start.fail_start = BackendStatus::IoError;
    CHECK(start_vm.start() == BackendStatus::IoError);
    // The backend reports that the VM never left its created state. Reconcile
    // the public state to PREPARED instead of lying that the guest is running.
    CHECK(start_vm.snapshot().state == VmLifecycleState::Prepared);
    return 0;
}
