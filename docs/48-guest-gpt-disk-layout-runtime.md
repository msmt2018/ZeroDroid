# 48. Guest GPT 磁盘布局与分区地址映射

## 1. 目标

将现有 zerodroid-image-core::gpt 的 GPT 解析结果转化为 Guest 磁盘运行时可用的分区目录和地址映射。解析器负责验证镜像元数据；本 crate 不重新实现 GPT 解析，也不把完整镜像复制到内存。

## 2. 新 crate

zerodroid-disk-layout-core 依赖现有 zerodroid-image-core，输入已验证的 GptImageIndex，输出不可变 GptDiskLayout：
- 保留分区 index、名称、type GUID、unique GUID、attributes、LBA 范围；
- 检查 sector size、磁盘容量、字节范围与解析结果一致；
- 拒绝重复分区名、重复 unique GUID、重叠分区和非法 UTF-16 名称；
- 提供按名称/GUID 查找；
- 将分区相对 LBA 映射为父虚拟磁盘 byte range；
- 拒绝越界、零长度、整数溢出和非法逻辑块大小。

映射返回的是虚拟磁盘内部偏移，不是宿主文件路径、物理分区编号或宿主块设备 FD。

## 3. 与 Block Core 的关系

上层可将 PartitionView 的 start_byte + length_bytes 用于创建有界分区视图，再通过 zerodroid-block-core 对 Guest 请求执行 generation 校验和读写。不能让分区视图绕过 BlockBackend 的 VM 隔离、只读策略、flush 和 discard 约束。

## 4. 当前限制

- 当前输入是已解析的 GptImageIndex；原有 GPT parser 目前以完整输入切片为接口，因此这并不等于已完成大镜像随机访问/分块 GPT 解析。
- 本 crate 不负责写回 GPT、更新 primary/backup header、CRC32 重算或原子提交。
- 尚未把 LP Metadata 的 linear/zero extents 转成运行时映射；动态分区是后续独立阶段。
- 不含 COW、Virtual A/B、VirtIO transport、Guest DMA 或 VMM 设备注册。

## 5. 测试

覆盖排序、分区名/GUID 查询、分区相对 LBA 地址映射、越界、重复名称/GUID、重叠范围和非法 UTF-16 名称。测试构造的是解析结果对象，不替代对真实 GPT 镜像、主备 GPT 一致性及真实 Guest I/O 的集成测试。