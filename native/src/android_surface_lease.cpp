#include <jni.h>
#include <android/native_window.h>
#include <android/native_window_jni.h>

#include <atomic>
#include <cstdint>
#include <memory>
#include <mutex>
#include <unordered_map>

namespace {
struct WindowDeleter {
    void operator()(ANativeWindow* window) const noexcept {
        if (window != nullptr) ANativeWindow_release(window);
    }
};
using WindowPtr = std::unique_ptr<ANativeWindow, WindowDeleter>;

std::mutex g_windows_mutex;
std::unordered_map<std::uint64_t, WindowPtr> g_windows;
std::atomic<std::uint64_t> g_next_handle{1};

void throw_illegal_argument(JNIEnv* env, const char* message) noexcept {
    jclass type = env->FindClass("java/lang/IllegalArgumentException");
    if (type != nullptr) env->ThrowNew(type, message);
}
void throw_illegal_state(JNIEnv* env, const char* message) noexcept {
    jclass type = env->FindClass("java/lang/IllegalStateException");
    if (type != nullptr) env->ThrowNew(type, message);
}

bool allocate_handle(std::uint64_t* out) noexcept {
    auto current = g_next_handle.load(std::memory_order_relaxed);
    while (current != 0 && current < static_cast<std::uint64_t>(INT64_MAX)) {
        if (g_next_handle.compare_exchange_weak(
                current, current + 1, std::memory_order_relaxed,
                std::memory_order_relaxed)) {
            *out = current;
            return true;
        }
    }
    return false;
}

jlongArray make_metrics(JNIEnv* env, std::uint64_t handle, ANativeWindow* window) {
    const jlong values[] = {
        static_cast<jlong>(handle),
        static_cast<jlong>(ANativeWindow_getWidth(window)),
        static_cast<jlong>(ANativeWindow_getHeight(window)),
        static_cast<jlong>(ANativeWindow_getFormat(window))
    };
    jlongArray result = env->NewLongArray(4);
    if (result != nullptr) env->SetLongArrayRegion(result, 0, 4, values);
    return result;
}
} // namespace

extern "C" JNIEXPORT jlongArray JNICALL
Java_dev_zerodroid_core_nativebridge_NativeRuntime_nativeAcquireSurface(
        JNIEnv* env, jobject, jobject surface) {
    if (surface == nullptr) {
        throw_illegal_argument(env, "Surface must not be null");
        return nullptr;
    }

    // ANativeWindow_fromSurface returns an acquired reference. The registry owns
    // exactly this reference until nativeReleaseSurface removes the lease.
    WindowPtr window(ANativeWindow_fromSurface(env, surface));
    if (!window) {
        if (!env->ExceptionCheck()) {
            throw_illegal_state(env, "Android could not acquire an ANativeWindow from Surface");
        }
        return nullptr;
    }

    std::uint64_t handle = 0;
    if (!allocate_handle(&handle)) {
        throw_illegal_state(env, "Native Surface handle space exhausted");
        return nullptr;
    }

    jlongArray metrics = nullptr;
    try {
        std::lock_guard<std::mutex> lock(g_windows_mutex);
        auto inserted = g_windows.emplace(handle, std::move(window));
        if (!inserted.second) {
            throw_illegal_state(env, "Native Surface handle collision");
            return nullptr;
        }
        metrics = make_metrics(env, handle, inserted.first->second.get());
        if (metrics == nullptr) {
            g_windows.erase(inserted.first);
            return nullptr;
        }
    } catch (const std::bad_alloc&) {
        throw_illegal_state(env, "Native Surface registry allocation failed");
        return nullptr;
    } catch (...) {
        throw_illegal_state(env, "Unexpected Native Surface registry failure");
        return nullptr;
    }
    return metrics;
}

extern "C" JNIEXPORT jlongArray JNICALL
Java_dev_zerodroid_core_nativebridge_NativeRuntime_nativeQuerySurface(
        JNIEnv* env, jobject, jlong raw_handle) {
    if (raw_handle <= 0) {
        throw_illegal_argument(env, "Surface lease handle must be positive");
        return nullptr;
    }
    std::lock_guard<std::mutex> lock(g_windows_mutex);
    const auto found = g_windows.find(static_cast<std::uint64_t>(raw_handle));
    if (found == g_windows.end()) {
        throw_illegal_state(env, "Surface lease is closed or stale");
        return nullptr;
    }
    return make_metrics(env, found->first, found->second.get());
}

extern "C" JNIEXPORT jboolean JNICALL
Java_dev_zerodroid_core_nativebridge_NativeRuntime_nativeReleaseSurface(
        JNIEnv*, jobject, jlong raw_handle) {
    if (raw_handle <= 0) return JNI_FALSE;
    WindowPtr released;
    {
        std::lock_guard<std::mutex> lock(g_windows_mutex);
        const auto found = g_windows.find(static_cast<std::uint64_t>(raw_handle));
        if (found == g_windows.end()) return JNI_FALSE;
        released = std::move(found->second);
        g_windows.erase(found);
    }
    // Drop the ANativeWindow reference outside the registry lock.
    released.reset();
    return JNI_TRUE;
}
