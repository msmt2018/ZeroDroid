# 46. Guest 虚拟分区存储与刷写事务核心

> 本文定义每台虚拟手机独立的分区存储与刷写事务。当前 Rust crate 提供有界事务协调核心和后端契约；尚未提供 Android 文件系统/块设备生产后端，也未声称已完成真实刷写。

## 1. 目标

每个 VM 必须拥有独立的虚拟分区命名空间。Guest 的 GPT、boot、vendor_boot、init_boot、vbmeta、super、userdata、metadata、misc、recovery 等分区均是 VM 私有存储对象。任何路径、文件描述符或分区标识都不得被解释为宿主物理分区路径。

存储层应支持：
- 分区目录和容量元数据；
- 稀疏镜像与 raw 镜像的流式写入；
- staging object / copy-on-write；
- 校验、提交、回滚与进程死亡后的 staging 清理；
- VM generation 隔离和 per-VM 配额；
- A/B slot 与动态分区元数据的原子更新。

## 2. Rust 核心

`zerodroid-partition-store` 提供平台无关的安全 Rust 核心：
- `VmId` + `generation` 作为操作作用域；
- `PartitionStore` trait 由真实后端实现，负责命名空间隔离、配额、锁、持久化和原子提交；
- `FlashTransaction::begin` 检查 VM generation、分区存在性、flash policy、镜像大小和分区容量；
- `write_chunk` 仅接受严格连续 offset，单块最大 1 MiB，拒绝空块、超大块、偏移跳跃和超出镜像边界；
- 后端 staging handle 是不透明关联类型，不能由协议输入直接构造；
- `commit` 要求完整镜像，并要求后端重新校验预期 SHA-256，再原子发布新分区映射；
- commit 失败或事务对象释放时尽力 abort；真实后端仍必须在启动时扫描并清理进程死亡后遗留的 staging 对象；
- erase 由后端原子替换，且必须单独通过分区 erasable policy。

核心不实现具体 SHA-256 算法，也不把调用方提供的 digest 当作可信事实。生产后端必须对 staging object 实际内容重新计算摘要，并与期望值比较。AVB/vbmeta 签名验证、rollback index 和分区内容格式验证仍是更高层启动安全职责。

## 3. 原子性与失败语义

生产后端的 `commit_staging` 必须满足：
1. staged image 完整、长度一致、摘要验证通过；
2. VM ID 与 generation 仍然有效；
3. 同一 VM + partition 的写入锁仍由本事务持有；
4. 原分区映射在 commit 前保持可用；
5. commit 使用原子元数据切换或等价的 crash-consistent 机制；
6. commit 失败时旧分区仍可读取；
7. commit 成功后 staging handle 不再可写、不可再次提交。

不得把多个普通文件写入包装成“原子事务”。建议以 VM 私有目录中的不可变镜像对象 + 小型原子 manifest/slot-map 替换来实现；文件与目录同步策略应针对目标 Android 文件系统验证。对于大型 super image，应使用 extent/COW 或稀疏 backing，不应在 RAM 中构建完整镜像。

## 4. 生命周期与安全

- VM destroy、reset、generation increment 必须使旧事务失效；
- backend 每次写入、verify、commit 都必须再次检查 generation；
- staging quota 应同时限制每 VM 总 staging bytes、对象数量、单镜像大小和文件系统可用空间；
- handle 必须绑定 VM/partition/generation，不允许跨 VM 使用；
- 分区名只允许受限 ASCII token，不接受路径分隔符；
- staging 文件必须使用应用私有目录、安全创建标志和不可预测名称；不跟随符号链接，不接受外部传入的文件路径；
- 进程崩溃后通过 journal/manifest 恢复，未提交对象只能被清理，不能自动成为活动分区；
- erase、slot switch 和 partition-map 更新必须和 flash 互斥。

## 5. 与虚拟 Fastboot 的连接

Fastboot core 的 `DownloadReady` token 应绑定到 backend staging session。每个 Fastboot chunk 到达后：
1. 通过 Fastboot token 查找 VM 私有 staging handle；
2. 检查传输层 packet 长度与当前 offset；
3. 调用 `FlashTransaction::write_chunk`；
4. 只有后端写入成功，才确认 Fastboot download offset；
5. 完整接收后 Fastboot 发出 FlashPlan；
6. 事务协调器验证镜像摘要/格式并 commit；
7. 返回 Fastboot OKAY/FAIL，随后释放 token 和 staging 元数据。

Fastboot 协议接收完成不等于分区已刷写完成；只有事务 commit 成功才可回复刷写成功。

## 6. 测试边界

单元测试覆盖有界分块、offset 单调性、容量/策略、generation、digest mismatch、失败提交保留旧分区和 Drop abort。内存测试后端仅用于测试事务状态机，不代表生产存储实现。

后续还需增加：
- 真正的文件后端 crash-consistency 与 fsync 测试；
- 并发 flash/erase 锁与 VM 隔离测试；
- 进程被杀、空间耗尽、短写、I/O 错误和恢复扫描测试；
- sparse image streaming materializer；
- GPT/LP metadata 的读写事务与一致性校验；
- A/B slot metadata 原子切换和 Virtual A/B COW snapshot。
