# 38. Boot Material Set 与启动材料组合索引

## 1. 目标

将已经通过各自格式解析器索引的 `boot.img`、`init_boot.img` 和 `vendor_boot.img` 组合为统一的 Boot Material Set。该层只负责跨镜像材料归集、来源标记和二次边界校验，不重新解析 payload，也不复制内核或 ramdisk 字节。

## 2. 材料角色

- `boot.img`: Kernel、Boot Ramdisk、可选 Boot Signature。
- `init_boot.img`: Init Ramdisk、可选 Init Boot Signature；当前入口只接受 Android Boot Header v4，且 kernel size 必须为 0、ramdisk 必须非空。
- `vendor_boot.img`: Vendor Ramdisk、DTB、v4 Vendor Ramdisk Table、v4 Bootconfig。

每个条目同时保留来源镜像与该镜像中的 byte range。调用方必须保证解析索引与传入字节切片来自同一份不可变镜像；构建器会再次检查每个 range 和 total indexed bytes 不越界。

## 3. v4 Vendor Ramdisk Fragment 选择

`vendor_boot` v4 表项解析会验证 fragment type（NONE/PLATFORM/RECOVERY/DLKM）、表项长度、payload 范围与 fragment 之间不重叠。`select_vendor_ramdisk_fragments` 按显式启动模式和目标 board ID 选择 range：Normal 选择匹配的 PLATFORM 与 DLKM；Recovery 选择匹配的 RECOVERY；fragment 的 board_id 非零字必须逐项匹配，零值作为通配。NONE 不会被自动拼入，避免在语义不明时猜测启动策略。选择结果只包含表项索引、类型、名称和原始镜像 range，不复制或拼接 ramdisk。

该选择器仍不是完整 Android bootloader 策略：目标 board ID 的权威来源、平台专属规则、fragment 最终排序、generic/init_boot ramdisk 与 vendor fragments 的组合顺序、bootconfig 参数冲突处理，必须由后续 Guest Platform Profile/boot policy 明确提供。不得把所选 range 直接当作已组装的可启动 ramdisk。 `build_selected_vendor_ramdisk_materials` 可将选择结果转换为独立的 `VendorRamdiskFragment` 材料条目，只引用被选中的 payload range；它不会把整个 vendor ramdisk blob 同时作为加载指令，也不会执行 cpio 合并。

## 4. Bootconfig 解析与合并边界

`bootconfig.rs` 当前实现有界的扁平 `key=value` 子集解析：限制输入字节、行长度、键/值长度和行数；允许空行与整行 `#` 注释；保留值文本，不做 shell 展开或类型推断；相同键相同值去重，相同键不同值直接报冲突。vendor_boot v4 的 `bootconfig` range 可通过 `parse_vendor_bootconfig` 解析。

当前明确拒绝嵌套节点、数组及其他未实现语法。`merge_bootconfig` 不擅自假定 bootloader/kernel/vendor 的覆盖优先级：重复键值相同才合并，值冲突必须交给后续显式 Boot Policy 解决。此实现不是完整通用 bootconfig grammar，也没有把参数写入 Kernel command line 或启动 Guest。

## 5. 资源与错误

构建接口要求调用方提供最大条目数。条目预算在每次追加前检查；越界索引、无效 init_boot 语义和预算耗尽分别返回明确错误。所有数据仍以 range 引用，不会将数 GB 的镜像或 ramdisk 整体复制到内存。

## 6. 验证状态

`BootMaterialSet.boot_chain_verified` 当前固定为 false。材料存在和结构可索引不等于：
- boot/vendor boot signature 已验证；
- AVB 公钥已受信任或 VBMeta 签名有效；
- DTB/DTBO 与目标 SoC/Guest machine 匹配；
- fstab、Vendor HAL、VINTF、SELinux 和分区布局兼容；
- Guest Kernel 能够成功启动。

DTBO、独立 DTB、Recovery、固件、AVB 信任链及 Guest Profile 的匹配仍需后续接入。不得仅凭 Boot Material Set 构建成功放行 VM 启动。
