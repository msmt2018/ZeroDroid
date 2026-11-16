#ifndef ZERODROID_RUST_CORE_H
#define ZERODROID_RUST_CORE_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
enum zdroid_rust_status {
 ZDROID_RUST_OK=0u, ZDROID_RUST_INVALID_ARGUMENT=1u, ZDROID_RUST_RESOURCE_LIMIT=2u,
 ZDROID_RUST_INVALID_IMAGE=3u, ZDROID_RUST_UNSUPPORTED=4u, ZDROID_RUST_INTERNAL=255u
};
typedef struct zdroid_rust_arm64_kernel_image_info {
 uint32_t struct_size; uint32_t page_size_code;
 uint64_t text_offset; uint64_t declared_image_size; uint64_t effective_image_size;
 uint64_t flags; uint64_t image_offset; uint64_t image_length;
} zdroid_rust_arm64_kernel_image_info;
typedef struct zdroid_rust_legacy_boot_image_info {
 uint32_t struct_size; uint32_t header_version; uint32_t header_size; uint32_t page_size;
 uint64_t kernel_offset; uint64_t kernel_size; uint64_t ramdisk_offset; uint64_t ramdisk_size;
 uint64_t second_offset; uint64_t second_size; uint64_t recovery_dtbo_offset; uint64_t recovery_dtbo_size;
 uint64_t dtb_offset; uint64_t dtb_size; uint32_t recovery_dtbo_present; uint32_t dtb_present;
 uint64_t dtb_address; uint64_t total_indexed_bytes;
} zdroid_rust_legacy_boot_image_info;
uint32_t zdroid_rust_parse_arm64_kernel_image(const uint8_t* data, size_t length, uint64_t max_input_bytes, uint64_t max_image_bytes, zdroid_rust_arm64_kernel_image_info* out_info);
uint32_t zdroid_rust_parse_legacy_boot_image(const uint8_t* data, size_t length, uint64_t max_input_bytes, uint64_t max_payload_bytes, zdroid_rust_legacy_boot_image_info* out_info);
#ifdef __cplusplus
}
#endif
#endif
