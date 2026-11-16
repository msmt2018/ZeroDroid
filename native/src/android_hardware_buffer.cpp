#include <jni.h>
#include <android/hardware_buffer.h>

#include <cstddef>
#include <cstdint>
#include <limits>
#include <memory>
#include <mutex>
#include <new>
#include <utility>
#include <unordered_map>

namespace {
struct HardwareBufferDeleter {
    void operator()(AHardwareBuffer* buffer) const noexcept {
        if (buffer != nullptr) AHardwareBuffer_release(buffer);
    }
};
using HardwareBufferPtr = std::unique_ptr<AHardwareBuffer, HardwareBufferDeleter>;

struct Entry {
    HardwareBufferPtr buffer;
    std::uint64_t accounted_bytes;
};

std::mutex g_mutex;
std::unordered_map<std::uint64_t, Entry> g_buffers;
std::uint64_t g_next_handle = 1;
std::uint64_t g_accounted_bytes = 0;
constexpr std::size_t kMaxLiveBuffers = 64;
constexpr std::uint64_t kMaxBufferBytes = 64ULL * 1024ULL * 1024ULL;
constexpr std::uint64_t kMaxTotalBytes = 256ULL * 1024ULL * 1024ULL;
constexpr std::uint32_t kMaxDimension = 8192;

constexpr std::uint64_t kAllowedUsage =
        AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE |
        AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT |
        AHARDWAREBUFFER_USAGE_CPU_READ_RARELY |
        AHARDWAREBUFFER_USAGE_CPU_READ_OFTEN |
        AHARDWAREBUFFER_USAGE_CPU_WRITE_RARELY |
        AHARDWAREBUFFER_USAGE_CPU_WRITE_OFTEN;
constexpr std::uint64_t kRequiredGpuUsage =
        AHARDWAREBUFFER_USAGE_GPU_SAMPLED_IMAGE |
        AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT;

void throw_argument(JNIEnv* env, const char* message) noexcept {
    jclass type = env->FindClass("java/lang/IllegalArgumentException");
    if (type != nullptr) env->ThrowNew(type, message);
}
void throw_state(JNIEnv* env, const char* message) noexcept {
    jclass type = env->FindClass("java/lang/IllegalStateException");
    if (type != nullptr) env->ThrowNew(type, message);
}

bool next_handle(std::uint64_t* result) noexcept {
    if (g_next_handle == 0 ||
        g_next_handle >= static_cast<std::uint64_t>(std::numeric_limits<std::int64_t>::max())) {
        return false;
    }
    *result = g_next_handle++;
    return true;
}

jlongArray describe(JNIEnv* env, std::uint64_t handle, AHardwareBuffer* buffer) {
    AHardwareBuffer_Desc desc{};
    AHardwareBuffer_describe(buffer, &desc);
    const jlong values[] = {
        static_cast<jlong>(handle),
        static_cast<jlong>(desc.width),
        static_cast<jlong>(desc.height),
        static_cast<jlong>(desc.layers),
        static_cast<jlong>(desc.format),
        static_cast<jlong>(desc.stride),
        static_cast<jlong>(desc.usage)
    };
    jlongArray array = env->NewLongArray(7);
    if (array != nullptr) env->SetLongArrayRegion(array, 0, 7, values);
    return array;
}
} // namespace

extern "C" JNIEXPORT jlongArray JNICALL
Java_dev_zerodroid_core_nativebridge_NativeRuntime_nativeAllocateHardwareBuffer(
        JNIEnv* env, jobject, jint width, jint height, jlong raw_usage) {
    if (width <= 0 || height <= 0 ||
        static_cast<std::uint32_t>(width) > kMaxDimension ||
        static_cast<std::uint32_t>(height) > kMaxDimension) {
        throw_argument(env, "HardwareBuffer dimensions must be within 1..8192");
        return nullptr;
    }
    const auto usage = static_cast<std::uint64_t>(raw_usage);
    if (raw_usage <= 0 || (usage & ~kAllowedUsage) != 0 ||
        (usage & kRequiredGpuUsage) != kRequiredGpuUsage) {
        throw_argument(env, "HardwareBuffer usage must contain sampled-image and color-output GPU flags and no unsupported bits");
        return nullptr;
    }

    AHardwareBuffer_Desc requested{};
    requested.width = static_cast<std::uint32_t>(width);
    requested.height = static_cast<std::uint32_t>(height);
    requested.layers = 1;
    requested.format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM;
    requested.usage = usage;

    AHardwareBuffer* raw = nullptr;
    const int status = AHardwareBuffer_allocate(&requested, &raw);
    if (status != 0 || raw == nullptr) {
        throw_state(env, "AHardwareBuffer allocation failed");
        return nullptr;
    }
    HardwareBufferPtr buffer(raw);

    AHardwareBuffer_Desc actual{};
    AHardwareBuffer_describe(buffer.get(), &actual);
    if (actual.width != requested.width || actual.height != requested.height ||
        actual.layers != 1 || actual.format != AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM ||
        (actual.usage & usage) != usage) {
        throw_state(env, "AHardwareBuffer allocator returned incompatible geometry, format or usage");
        return nullptr;
    }

    constexpr std::uint64_t kBytesPerPixel = 4;
    if (actual.stride == 0 ||
        actual.height > std::numeric_limits<std::uint64_t>::max() / actual.stride ||
        static_cast<std::uint64_t>(actual.stride) * actual.height >
                std::numeric_limits<std::uint64_t>::max() / kBytesPerPixel) {
        throw_state(env, "AHardwareBuffer allocation size overflow");
        return nullptr;
    }
    const std::uint64_t bytes =
            static_cast<std::uint64_t>(actual.stride) * actual.height * kBytesPerPixel;
    if (bytes > kMaxBufferBytes) {
        throw_state(env, "AHardwareBuffer exceeds the 64 MiB per-buffer budget");
        return nullptr;
    }

    std::uint64_t handle = 0;
    {
        std::lock_guard<std::mutex> lock(g_mutex);
        if (g_buffers.size() >= kMaxLiveBuffers ||
            bytes > kMaxTotalBytes - g_accounted_bytes) {
            throw_state(env, "AHardwareBuffer registry count or memory budget exhausted");
            return nullptr;
        }
        if (!next_handle(&handle)) {
            throw_state(env, "AHardwareBuffer handle space exhausted");
            return nullptr;
        }
        try {
            const auto inserted = g_buffers.emplace(
                    handle, Entry{std::move(buffer), bytes});
            if (!inserted.second) {
                throw_state(env, "AHardwareBuffer handle collision");
                return nullptr;
            }
            g_accounted_bytes += bytes;
        } catch (const std::bad_alloc&) {
            throw_state(env, "AHardwareBuffer registry allocation failed");
            return nullptr;
        } catch (...) {
            throw_state(env, "Unexpected AHardwareBuffer registry failure");
            return nullptr;
        }
    }
    // Take a temporary reference while the registry is locked. Describe/JNI
    // allocation then runs without holding the registry mutex.
    HardwareBufferPtr retained;
    {
        std::lock_guard<std::mutex> lock(g_mutex);
        const auto found = g_buffers.find(handle);
        if (found == g_buffers.end()) {
            throw_state(env, "Newly registered HardwareBuffer disappeared");
            return nullptr;
        }
        AHardwareBuffer_acquire(found->second.buffer.get());
        retained.reset(found->second.buffer.get());
    }
    jlongArray result = describe(env, handle, retained.get());
    if (result == nullptr) {
        std::lock_guard<std::mutex> lock(g_mutex);
        const auto found = g_buffers.find(handle);
        if (found != g_buffers.end()) {
            g_accounted_bytes -= found->second.accounted_bytes;
            g_buffers.erase(found);
        }
    }
    return result;
}

extern "C" JNIEXPORT jlongArray JNICALL
Java_dev_zerodroid_core_nativebridge_NativeRuntime_nativeDescribeHardwareBuffer(
        JNIEnv* env, jobject, jlong raw_handle) {
    if (raw_handle <= 0) {
        throw_argument(env, "HardwareBuffer lease handle must be positive");
        return nullptr;
    }
    HardwareBufferPtr retained;
    {
        std::lock_guard<std::mutex> lock(g_mutex);
        const auto found = g_buffers.find(static_cast<std::uint64_t>(raw_handle));
        if (found == g_buffers.end()) {
            throw_state(env, "HardwareBuffer lease is closed or stale");
            return nullptr;
        }
        AHardwareBuffer_acquire(found->second.buffer.get());
        retained.reset(found->second.buffer.get());
    }
    return describe(env, static_cast<std::uint64_t>(raw_handle), retained.get());
}

extern "C" JNIEXPORT jboolean JNICALL
Java_dev_zerodroid_core_nativebridge_NativeRuntime_nativeReleaseHardwareBuffer(
        JNIEnv*, jobject, jlong raw_handle) {
    if (raw_handle <= 0) return JNI_FALSE;
    HardwareBufferPtr released;
    {
        std::lock_guard<std::mutex> lock(g_mutex);
        const auto found = g_buffers.find(static_cast<std::uint64_t>(raw_handle));
        if (found == g_buffers.end()) return JNI_FALSE;
        g_accounted_bytes -= found->second.accounted_bytes;
        released = std::move(found->second.buffer);
        g_buffers.erase(found);
    }
    released.reset();
    return JNI_TRUE;
}
