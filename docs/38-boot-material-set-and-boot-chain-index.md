# 38. Boot Material Set 与启动材料组合索引

## 1. 目标

将已经通过各自格式解析器索引的 `boot.img`、`init_boot.img` 和 `vendor_boot.img` 组合为统一的 Boot Material Set。该层只负责跨镜像材料归集、来源标记和二次边界校验，不重新解析 payload，也不复制内核或 ramdisk 字节。

## 2. 材料角色

- `boot.img`: Kernel、Boot Ramdisk、可选 Boot Signature。
- `init_boot.img`: Init Ramdisk、可选 Init Boot Signature；当前入口只接受 Android Boot Header v4，且 kernel size 必须为 0、ramdisk 必须非空。
- `vendor_boot.img`: Vendor Ramdisk、DTB、v4 Vendor Ramdisk Table、v4 Bootconfig。

每个条目同时保留来源镜像与该镜像中的 byte range。调用方必须保证解析索引与传入字节切片来自同一份不可变镜像；构建器会再次检查每个 range 和 total indexed bytes 不越界。

## 3. 资源与错误

构建接口要求调用方提供最大条目数。条目预算在每次追加前检查；越界索引、无效 init_boot 语义和预算耗尽分别返回明确错误。所有数据仍以 range 引用，不会将数 GB 的镜像或 ramdisk 整体复制到内存。

## 4. 验证状态

`BootMaterialSet.boot_chain_verified` 当前固定为 false。材料存在和结构可索引不等于：
- boot/vendor boot signature 已验证；
- AVB 公钥已受信任或 VBMeta 签名有效；
- DTB/DTBO 与目标 SoC/Guest machine 匹配；
- fstab、Vendor HAL、VINTF、SELinux 和分区布局兼容；
- Guest Kernel 能够成功启动。

DTBO、独立 DTB、Recovery、固件、AVB 信任链及 Guest Profile 的匹配仍需后续接入。不得仅凭 Boot Material Set 构建成功放行 VM 启动。
