# 47. Guest 虚拟块设备核心契约

> 本文定义 Guest 可见块设备的有界 I/O 核心。当前 crate 是平台无关的安全 Rust 层，不包含 VirtIO PCI/MMIO transport、Android 文件后端或真实 KVM 设备注册；不能将本契约描述为已经可启动 Guest 的块设备。

## 1. 职责边界

Guest Block Device 将 VM 私有分区/磁盘后端暴露为 Guest 可寻址的逻辑块设备。它不解析 GPT、不理解 Android 分区名，也不访问宿主物理分区。GPT/LP 分区图与虚拟磁盘映射由上层布局引擎负责。

`zerodroid-block-core` 负责：
- 校验逻辑块大小、容量、最大传输长度、队列深度和 discard 几何；
- 将 Guest LBA 转换为经过溢出检查的字节偏移；
- 拒绝零长度、非块对齐、越界、超长和只读写入；
- 在每次 I/O 前检查 VM generation；
- 将 read/write/flush/discard 明确委派给后端；
- 限制 discard 的粒度、范围与传输大小。

## 2. Geometry

`BlockGeometry` 包含 capacity_blocks、logical_block_size、max_transfer_bytes、queue_depth、read_only、discard_supported 和 discard_granularity_blocks。

当前保守硬上限：
- 逻辑块：512 B 至 64 KiB，且必须为 2 的幂；
- 单次传输：不超过 16 MiB，且为逻辑块大小的整数倍；
- queue depth：1 至 256；
- 容量乘法和 LBA 范围必须通过 checked arithmetic；
- discard 必须显式声明支持，且起始 LBA 和长度满足 discard granularity。

具体 VirtIO blk feature bits、flush/discard 语义、最大 segment 数和队列参数由未来 transport 层协商，不能直接把这些字段当作 VirtIO descriptor 规范。

## 3. Backend contract

`BlockBackend` 由 VM 私有存储实现：
- `check_generation`：拒绝 VM reset/destroy 后的旧设备请求；
- `read_at` / `write_at`：严格处理完整范围，不允许短读/短写伪装成功；
- `flush`：必须保证此前已确认写入按后端持久性语义完成；
- `discard`：只作用于该 VM 的虚拟存储对象，不能调用宿主物理 discard；
- 多队列/多线程场景必须在后端提供一致性锁或序列化策略，确保重叠写、flush 与 discard 的顺序正确。

I/O 核心以可变后端引用串行调用。后续 VirtIO transport 若引入并发队列，必须在后端共享层明确锁粒度、取消和队列停止行为，不能简单复制多个不一致的后端实例。

## 4. VM 隔离与生命周期

设备实例绑定 `VmId + generation`。VM 重启、销毁或替换磁盘时必须递增 generation 并撤销旧 transport 队列。后端仍需在操作时检查 generation，不能只依赖创建设备时检查一次。

每台 VM 的磁盘映射应引用 VM 私有的虚拟磁盘对象；不可接受用户输入路径、宿主块设备节点或未经授权的文件描述符作为 Guest 分区目标。

## 5. 与 Fastboot / Partition Store 的连接

Fastboot 事务负责把完整镜像写入 staging，再通过校验和原子 commit 更新分区对象。Block Backend 读取当前已提交的虚拟磁盘视图。不能让 Guest 在事务尚未提交时看到半写入镜像。

推荐下一步由 Virtual Disk Layer 把：
- GPT primary/backup header 与 partition entries；
- raw/sparse materialized extents；
- LP dynamic partition 映射；
- per-VM userdata 与 metadata；
- COW snapshot / A/B slot view

组合为一个或多个稳定虚拟磁盘对象，再交给 VirtIO Block transport。

## 6. 尚未实现的内容

- VirtIO block MMIO/PCI transport、descriptor chain 解析和 used ring；
- Guest DMA 地址到受控 Guest RAM 的校验映射；
- 多队列异步执行、eventfd/irq 注入和中断合并；
- Android 文件/块存储后端、稀疏文件 extent 映射和 COW；
- GPT/LP 到 block device 的实际映射；
- 真机 AVF/KVM VM 中的 Guest 驱动枚举与启动验证。

## 7. 测试

单元测试覆盖 LBA 读写、空/未对齐/超限/越界请求、只读、generation 失效、flush/discard 委派及 discard 几何。内存后端仅用于验证边界和调用语义，不代表持久化、崩溃一致性或设备性能已经验证。
