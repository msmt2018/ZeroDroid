# 45. Virtual Fastboot、Guest Bootloader 与虚拟分区刷写契约

> 本文定义虚拟 Fastboot 的协议核心和刷写安全边界。命令解析/刷写计划核心已经实现；USB/TCP 传输、Guest Bootloader、虚拟分区后端、原子提交和真实 Guest 启动尚未实现。

## 1. 目标与边界

ZeroDroid 的 Fastboot 是 Virtual Phone Instance 内部的刷机控制面。它操作该 VM 独立的虚拟 GPT/动态分区/虚拟磁盘，不得映射到宿主手机真实的 boot、recovery、super、userdata 或其他物理分区。

目标路径：

Host App / VM Manager
→ VM lifecycle controller
→ virtual bootloader state
→ Fastboot transport endpoint
→ bounded Fastboot protocol core
→ partition policy / flash transaction coordinator
→ per-VM virtual block store
→ Guest boot chain

Fastboot 不是 Hypervisor，也不能替代 VMM、Guest Kernel 或 AVB。进入 Fastboot 模式意味着虚拟机暂停正常 Guest 启动并切换到其虚拟 Bootloader 控制面；它不应将宿主设备重启到物理 Bootloader。

## 2. 命令核心与资源边界

Rust crate `zerodroid-fastboot-core` 提供无 unsafe、平台无关的协议核心：
- 有界 ASCII 命令解析，限制命令长度；
- 解析 getvar、download、flash、erase、set_active、reboot、reboot-bootloader 和 continue；当前核心按 bootloader Fastboot 语义报告 is-userspace=no，fastbootd 是独立后续模式；
- download 长度必须是 8 位十六进制值，且受策略上限约束；
- 下载不把镜像缓存在进程 RAM：协议核心发放 download token 和连续写入 reservation，传输层将数据流写入该 VM 的私有 staging object，持久化成功后再确认偏移；flash 必须等待精确长度完整；
- partition 名称限制为小写 ASCII 字母、数字、下划线、连字符和点，拒绝路径、斜线、大小写混淆；
- flash/erase 只能访问显式 PartitionDescriptor allowlist；
- 镜像大小不得超过虚拟分区容量；
- 分区目录条目数和 getvar 响应长度均有硬上限；
- slot 切换必须由策略显式启用；未启用时返回拒绝；
- getvar 只报告虚拟分区信息，不探测或泄露宿主物理分区。

协议核心输出 FlashPlan/ErasePlan。FlashPlan 只携带 session download token 与镜像长度，不携带整份镜像字节；传输层必须将 token 绑定到不可跨 VM 访问的临时存储对象。它们是待执行的操作描述，不代表分区已被修改。

## 3. 分区写入事务（下一阶段实现要求）

实际后端必须提供 per-VM transaction，建议接口语义：
1. begin(partition, expected_size, vm_generation, image_digest)
2. write(offset, bytes) —— 严格顺序或明确支持的随机写；所有偏移/长度 checked arithmetic
3. verify(expected_size, digest, format/profile constraints)
4. commit —— 仅在完整验证成功后原子切换当前分区映射
5. abort —— 删除临时对象并保留原分区可用版本

要求：
- 先写临时 backing object / COW extent，不允许直接覆盖当前可启动分区；
- 写入前检查分区 allowlist、分区容量、总 VM 存储配额、可用空间、镜像哈希和 VM generation；
- commit 使用原子元数据切换或后端保证的等价事务；不能把多次普通 write 伪称原子刷写；
- 失败、取消、进程死亡、磁盘满、VM 被销毁或 generation 变化时 abort；
- 防止同一分区并发 flash/erase；锁必须按 VM + partition 隔离；
- 完成后重新计算镜像哈希并记录审计事件；
- 所有分区 backing files 都位于该 VM 的私有存储命名空间，永不接收任意 Host 路径。

## 4. Guest Bootloader 与启动链

虚拟启动流程应由 VM Boot Orchestrator 管理：
- 选择 Boot Profile 和虚拟板级硬件配置；
- 验证 Boot Material Set 来源、哈希、版本、分区映射与 Guest Profile；
- 验证 AVB/vbmeta 信任链和 rollback policy（尚未实现完整 AVB 验证时必须阻止安全模式启动）；
- 根据启动模式选择虚拟 Bootloader Fastboot、Recovery、正常启动或诊断控制台；
- 从虚拟分区读取 boot/vendor_boot/init_boot、DTB/DTBO、vbmeta 和动态分区；
- 生成与 VMM 实际设备完全一致的 Guest FDT、bootconfig 和内核参数；
- 加载 Guest ARM64 Kernel 并交给硬件虚拟化 VMM 启动；
- 记录 firmware → bootloader → kernel → first-stage init → second-stage init → HAL/VINTF → Zygote → SystemServer → Launcher 的阶段状态。

pvmfw 是 AVF pVM 信任链组件，不是通用虚拟 Bootloader。普通 VM 的虚拟 Bootloader 与 AVF pvmfw 必须分开建模。

## 5. Fastboot 协议传输

协议 core 不绑定物理 USB、TCP 或 Android USB Gadget。后续传输适配器应分别定义：
- USB Fastboot transport：仅在 Android Host 设备权限、USB gadget/host 模式和设备路由真实支持时使用；
- 本地 IPC transport：VM 管理 UI 与后台 VM Service 的本机控制；
- 可选调试 TCP transport：默认关闭，仅在明确授权、认证、隔离和防重放保护后启用。

传输层负责 packet framing、超时、取消、短读/短写和连接断开；协议层不得假设一次 read 就对应一条完整命令或完整镜像。

## 6. 兼容性与安全

- 未知 OEM 命令默认拒绝，不执行 shell、不运行镜像内脚本、不透传任意 ioctl。
- 不支持 erase 的分区必须拒绝 erase；boot-critical 分区策略可进一步要求双确认/签名验证。
- 不得把宿主 Fastboot/ADB 服务或物理设备分区作为虚拟后端。
- 镜像导入和刷写使用流式 I/O、配额和可恢复临时文件；不能把多 GB super.img 无限制读入 RAM。
- 对 sparse、GPT、LP metadata、AVB、boot image、OTA 等输入复用 image-core 的有界解析器；解析成功不等于签名可信或可启动。
- 对下载数据、命令、分区元数据进行 fuzz 和属性测试。

## 7. 当前实现状态

已实现：
- 有界 Fastboot 命令 parser；
- download 长度限制、单调 token、连续 offset reservation 与分块确认状态，不在协议核心中缓存整份镜像；
- 虚拟分区 allowlist、容量约束、flash/erase plan；
- getvar 虚拟分区信息；
- 显式 slot switching policy；
- malformed command、路径注入、下载溢出、分区容量、slot policy 的单元测试。

未实现：
- USB/TCP/IPC transport；
- Guest Bootloader 固件或其 ARM64 执行环境；
- flash plan 到虚拟块设备的事务执行器；
- 临时 backing object、COW、原子 commit/abort 和掉电恢复；
- A/B slot metadata 与 Virtual A/B snapshot/COW；
- AVB 真正的密码学验证、rollback index 持久化；
- 虚拟 Recovery、fastbootd 动态分区语义；
- 与真实 VMM/Guest Kernel 的启动/重启集成。

CI 通过只证明协议核心代码的构建和单元测试；不证明虚拟 Fastboot 已能刷写或启动 Android Guest。
