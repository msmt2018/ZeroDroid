#include <jni.h>

#include <cerrno>
#include <cstddef>
#include <chrono>
#include <cstdint>
#include <fcntl.h>
#include <limits>
#include <mutex>
#include <poll.h>
#include <unordered_map>
#include <unistd.h>
#include <utility>

namespace {
class OwnedFenceFd {
public:
    explicit OwnedFenceFd(int fd = -1) noexcept : fd_(fd) {}
    OwnedFenceFd(const OwnedFenceFd&) = delete;
    OwnedFenceFd& operator=(const OwnedFenceFd&) = delete;
    OwnedFenceFd(OwnedFenceFd&& other) noexcept : fd_(other.fd_) { other.fd_ = -1; }
    OwnedFenceFd& operator=(OwnedFenceFd&& other) noexcept {
        if (this != &other) {
            reset();
            fd_ = other.fd_;
            other.fd_ = -1;
        }
        return *this;
    }
    ~OwnedFenceFd() { reset(); }
    int get() const noexcept { return fd_; }
    void reset() noexcept {
        if (fd_ >= 0) {
            // On Linux, do not retry close() after EINTR: the descriptor may
            // already have been released and reused by another thread.
            (void)::close(fd_);
            fd_ = -1;
        }
    }
private:
    int fd_;
};

std::mutex g_mutex;
std::unordered_map<std::uint64_t, OwnedFenceFd> g_fences;
std::uint64_t g_next_handle = 1;
constexpr std::size_t kMaxLiveFences = 128;
constexpr int kMaxWaitMillis = 60'000;

void throw_argument(JNIEnv* env, const char* message) noexcept {
    jclass type = env->FindClass("java/lang/IllegalArgumentException");
    if (type != nullptr) env->ThrowNew(type, message);
}
void throw_state(JNIEnv* env, const char* message) noexcept {
    jclass type = env->FindClass("java/lang/IllegalStateException");
    if (type != nullptr) env->ThrowNew(type, message);
}

bool allocate_handle(std::uint64_t* out) noexcept {
    if (g_next_handle == 0 ||
        g_next_handle >= static_cast<std::uint64_t>(std::numeric_limits<jlong>::max())) {
        return false;
    }
    *out = g_next_handle++;
    return true;
}

int duplicate_cloexec(int fd) noexcept {
    int result;
    do {
        result = ::fcntl(fd, F_DUPFD_CLOEXEC, 0);
    } while (result < 0 && errno == EINTR);
    return result;
}

// Returns 1 when signaled, 0 on timeout, and -1 on an invalid/error condition.
// A negative fd is the standard already-signaled native-fence representation.
int wait_fence_fd(int fd, int timeout_ms) noexcept {
    if (fd < 0) return 1;
    pollfd descriptor{};
    descriptor.fd = fd;
    descriptor.events = POLLIN;
    const auto deadline = std::chrono::steady_clock::now() +
            std::chrono::milliseconds(timeout_ms);
    int remaining = timeout_ms;
    for (;;) {
        const int result = ::poll(&descriptor, 1, remaining);
        if (result == 0) return 0;
        if (result < 0 && errno == EINTR) {
            const auto now = std::chrono::steady_clock::now();
            if (now >= deadline) return 0;
            const auto left = std::chrono::duration_cast<std::chrono::milliseconds>(
                    deadline - now).count();
            remaining = static_cast<int>(left > 0 ? left : 0);
            continue;
        }
        if (result < 0) return -1;
        if ((descriptor.revents & POLLIN) != 0) return 1;
        return -1;
    }
}
} // namespace

extern "C" JNIEXPORT jlong JNICALL
Java_dev_zerodroid_core_nativebridge_NativeRuntime_nativeImportFenceFd(
        JNIEnv* env, jobject, jint source_fd) {
    if (source_fd < -1) {
        throw_argument(env, "Native fence fd must be -1 (already signaled) or a valid descriptor");
        return 0;
    }
    OwnedFenceFd owned;
    if (source_fd >= 0) {
        const int duplicate = duplicate_cloexec(source_fd);
        if (duplicate < 0) {
            throw_state(env, "Failed to duplicate native fence fd with close-on-exec");
            return 0;
        }
        owned = OwnedFenceFd(duplicate);
    }

    std::lock_guard<std::mutex> lock(g_mutex);
    if (g_fences.size() >= kMaxLiveFences) {
        throw_state(env, "Native fence lease limit reached");
        return 0;
    }
    std::uint64_t handle = 0;
    if (!allocate_handle(&handle)) {
        throw_state(env, "Native fence handle space exhausted");
        return 0;
    }
    try {
        const auto inserted = g_fences.emplace(handle, std::move(owned));
        if (!inserted.second) {
            throw_state(env, "Native fence handle collision");
            return 0;
        }
    } catch (...) {
        throw_state(env, "Native fence registry allocation failed");
        return 0;
    }
    return static_cast<jlong>(handle);
}

extern "C" JNIEXPORT jint JNICALL
Java_dev_zerodroid_core_nativebridge_NativeRuntime_nativeWaitFence(
        JNIEnv* env, jobject, jlong raw_handle, jint timeout_ms) {
    if (raw_handle <= 0 || timeout_ms < 0 || timeout_ms > kMaxWaitMillis) {
        throw_argument(env, "Invalid native fence handle or wait timeout (0..60000 ms)");
        return -1;
    }

    int fd = -1;
    OwnedFenceFd retained;
    {
        std::lock_guard<std::mutex> lock(g_mutex);
        const auto found = g_fences.find(static_cast<std::uint64_t>(raw_handle));
        if (found == g_fences.end()) {
            throw_state(env, "Native fence lease is closed or stale");
            return -1;
        }
        if (found->second.get() >= 0) {
            fd = duplicate_cloexec(found->second.get());
            if (fd < 0) {
                throw_state(env, "Failed to duplicate native fence for wait");
                return -1;
            }
            retained = OwnedFenceFd(fd);
        }
    }
    const int result = wait_fence_fd(retained.get(), timeout_ms);
    if (result < 0) {
        throw_state(env, "Native fence poll failed or returned an error state");
        return -1;
    }
    return result;
}

extern "C" JNIEXPORT jboolean JNICALL
Java_dev_zerodroid_core_nativebridge_NativeRuntime_nativeReleaseFence(
        JNIEnv*, jobject, jlong raw_handle) {
    if (raw_handle <= 0) return JNI_FALSE;
    OwnedFenceFd released;
    {
        std::lock_guard<std::mutex> lock(g_mutex);
        const auto found = g_fences.find(static_cast<std::uint64_t>(raw_handle));
        if (found == g_fences.end()) return JNI_FALSE;
        released = std::move(found->second);
        g_fences.erase(found);
    }
    released = OwnedFenceFd();
    return JNI_TRUE;
}
