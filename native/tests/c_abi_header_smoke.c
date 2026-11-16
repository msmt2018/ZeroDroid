#include "zerodroid/zerodroid.h"
#include "zerodroid/rust_core.h"
#include <stddef.h>
#include <stdint.h>
_Static_assert(sizeof(zdroid_handle)==8,"zdroid_handle ABI must be 64-bit");
_Static_assert(offsetof(zdroid_runtime_options,abi_version)==0,"ABI version offset changed");
_Static_assert(offsetof(zdroid_runtime_options,struct_size)==4,"struct size offset changed");
_Static_assert(offsetof(zdroid_error_info,operation_id)==16,"error layout changed");
_Static_assert(ZDROID_ABI_VERSION==2u,"native capability ABI must be version 2");
_Static_assert(sizeof(zdroid_rust_arm64_kernel_image_info)==56,"Rust ARM64 image ABI layout changed");
_Static_assert(sizeof(zdroid_rust_legacy_boot_image_info)==120,"Rust legacy boot ABI layout changed");
_Static_assert(offsetof(zdroid_host_capabilities,vm_create_state)==24,"VM creation state ABI offset changed");
_Static_assert(offsetof(zdroid_host_capabilities,vm_create_errno)==28,"VM creation errno ABI offset changed");
int main(void){if(zdroid_get_abi_version()!=ZDROID_ABI_VERSION)return 1;if(zdroid_status_name(ZDROID_STATUS_OK)==0)return 2;return 0;}
