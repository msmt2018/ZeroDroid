#include <jni.h>
#include "zerodroid/zerodroid.h"
#include "zerodroid/rust_core.h"
#include <cstdint>
#include <limits>
extern "C" JNIEXPORT jint JNICALL
Java_dev_zerodroid_core_nativebridge_NativeRuntime_abiVersion(JNIEnv*, jobject) {
 return static_cast<jint>(zdroid_get_abi_version());
}

extern "C" JNIEXPORT jintArray JNICALL
Java_dev_zerodroid_core_nativebridge_NativeRuntime_probeHostCapabilitiesRaw(JNIEnv* env, jobject) {
 zdroid_host_capabilities capabilities{};
 capabilities.struct_size = sizeof(capabilities);
 zdroid_error_info error{};
 error.struct_size = sizeof(error);
 if (zdroid_host_probe_capabilities(&capabilities, &error) != ZDROID_STATUS_OK) return nullptr;
 const jint values[] = {
  static_cast<jint>(capabilities.abi_version),
  static_cast<jint>(capabilities.architecture),
  static_cast<jint>(capabilities.kvm_state),
  static_cast<jint>(capabilities.kvm_api_version),
  static_cast<jint>(capabilities.kvm_errno),
  static_cast<jint>(capabilities.vm_create_state),
  static_cast<jint>(capabilities.vm_create_errno)
 };
 jintArray result = env->NewIntArray(7);
 if (result == nullptr) return nullptr;
 env->SetIntArrayRegion(result, 0, 7, values);
 return result;
}

extern "C" JNIEXPORT jlongArray JNICALL
Java_dev_zerodroid_core_nativebridge_NativeRuntime_parseArm64KernelImageRaw(
    JNIEnv* env, jobject, jobject direct_buffer, jlong max_input_bytes, jlong max_image_bytes) {
    if (direct_buffer == nullptr || max_input_bytes <= 0 || max_image_bytes <= 0) {
        return nullptr;
    }
    void* address = env->GetDirectBufferAddress(direct_buffer);
    const jlong capacity = env->GetDirectBufferCapacity(direct_buffer);
    if (address == nullptr || capacity <= 0 ||
        static_cast<unsigned long long>(capacity) >
            static_cast<unsigned long long>(std::numeric_limits<std::size_t>::max())) {
        return nullptr;
    }

    zdroid_rust_arm64_kernel_image_info info{};
    info.struct_size = sizeof(info);
    const auto status = zdroid_rust_parse_arm64_kernel_image(
        static_cast<const std::uint8_t*>(address),
        static_cast<std::size_t>(capacity),
        static_cast<std::uint64_t>(max_input_bytes),
        static_cast<std::uint64_t>(max_image_bytes),
        &info);

    const jlong values[] = {
        static_cast<jlong>(status),
        static_cast<jlong>(info.page_size_code),
        static_cast<jlong>(info.text_offset),
        static_cast<jlong>(info.declared_image_size),
        static_cast<jlong>(info.effective_image_size),
        static_cast<jlong>(info.flags),
        static_cast<jlong>(info.image_offset),
        static_cast<jlong>(info.image_length),
    };
    jlongArray result = env->NewLongArray(8);
    if (result == nullptr) return nullptr;
    env->SetLongArrayRegion(result, 0, 8, values);
    return result;
}
