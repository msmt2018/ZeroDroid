# 49. Guest Dynamic Partition 运行时映射

## 1. 目标

复用 zerodroid-image-core::lp_metadata 的 liblp 解析结果，将 Android Dynamic Partition 的逻辑地址转换为 VM 私有 super 虚拟磁盘上的有界 extent 地址。该层不重复解析 liblp，也不直接执行宿主文件 I/O。

## 2. 新 crate

zerodroid-dynamic-partition-core 输入 LpMetadataIndex，构建 DynamicPartitionLayout：
- 保留 metadata slot、逻辑分区名称、属性、group 和逻辑容量；
- 根据 first_extent_index / num_extents 读取已验证的 extent 序列；
- 将 linear extent 转换为 block-device index、设备名和字节偏移；
- 将 zero extent 表示为逻辑零区间，不分配与 extent 大小相同的内存；
- 验证 extent 总逻辑长度与 partition.logical_sectors 一致；
- 对跨 extent 的请求返回多个有界 segment，并提供 max_segments 限制。

## 3. 地址语义

liblp extent 的扇区单位固定按 512 bytes 计算。Linear target_data 是目标 block device 中的扇区位置；运行时输出的是该 VM 虚拟 block device 内部 byte offset。它不是宿主物理设备地址、宿主文件路径或 FD。

Zero extent 不产生后端读取：读操作应返回零填充；写入策略由上层 snapshot/COW 决定，不能把 zero extent 直接写入共享宿主存储。

## 4. 与 Guest Block Core 的连接

后续 Virtual Disk Backend 应根据 block_device_index/name 绑定 VM 私有 backing object，再把每个 linear segment 映射到受限 backend read/write。跨 extent 请求必须逐段完成，任一段失败都不能向 Guest 报告整条请求成功。Flush 必须传递到涉及的 backing objects；Discard 必须遵守 extent 类型、COW 状态和底层 discard 粒度。

## 5. 安全和资源边界

- map_range 接受 max_segments，防止恶意或异常请求导致无界 segment vector；
- 只允许请求完全落在一个逻辑分区容量内；
- 所有 sector/byte 转换和区间结束使用 checked arithmetic；
- VM generation、后端隔离、并发写入和持久化仍由 BlockBackend/Virtual Disk Backend 负责；
- 不接受用户输入宿主路径，也不暴露宿主物理分区。

## 6. 当前限制

- 输入必须是已解析的 LpMetadataIndex；当前 liblp parser 仍接收完整 super 镜像切片。
- 本 crate 只构建只读映射元数据，不执行实际 I/O，也未接入 BlockBackend。
- 尚未实现 LP metadata 写入、slot 更新、COW、Virtual A/B、snapshot 合并或 crash recovery。
- 尚未完成真实 Guest Kernel / VirtIO Block / ARM64 VMM 集成验证。

## 7. 测试

覆盖 linear/zero/linear 跨 extent 映射、逻辑范围越界、零长度、segment 数量上限、分区 extent 总长度不一致和目标 super block device 越界。测试使用内存构造的 LpMetadataIndex，不替代真实 super 镜像与设备启动验证。