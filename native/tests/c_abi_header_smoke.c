#include "zerodroid/zerodroid.h"
#include <stddef.h>
#include <stdint.h>
_Static_assert(sizeof(zdroid_handle)==8,"zdroid_handle ABI must be 64-bit");
_Static_assert(offsetof(zdroid_runtime_options,abi_version)==0,"ABI version offset changed");
_Static_assert(offsetof(zdroid_runtime_options,struct_size)==4,"struct size offset changed");
_Static_assert(offsetof(zdroid_error_info,operation_id)==16,"error layout changed");
int main(void){if(zdroid_get_abi_version()!=ZDROID_ABI_VERSION)return 1;if(zdroid_status_name(ZDROID_STATUS_OK)==0)return 2;return 0;}
