# 42. Android AVF 技术基准、ZeroDroid 架构校准与完整开发路线

> 本文以 Android 官方 AVF 文档为技术基准，定义 ZeroDroid 的生产架构、实现顺序、验收门槛和未实现边界。本文是工程计划，不代表所列功能已经实现。

## 1. 目标与强制约束

ZeroDroid 的目标是在 Android ARM64 手机/平板上运行独立 Guest 系统。Guest 必须拥有独立 ARM64 Kernel、Guest 物理地址空间、虚拟中断/定时器、虚拟设备、独立存储和可验证的启动链。它不是容器、chroot、proot，也不是 Host Kernel 上的一组普通进程。

生产执行路径：

Android Host → 授权 AVF 或系统授权 ARM64 KVM/pKVM → VMM → vCPU / Guest memory / virtual interrupt controller / VirtIO → Guest ARM64 Kernel → Guest init / Android userspace

强制约束：
1. 不引入 QEMU、TCG、Android Emulator、Goldfish、AEMU 或 Cuttlefish 模拟器作为生产运行路径，也不提供软件 CPU 模拟 fallback。
2. crosvm 是 VMM，不是 Hypervisor。可以逐模块审计、裁剪、复用或借鉴其 Rust/VirtIO/KVM 实现；编译通过不代表 Android 运行适配完成。
3. AVF 是优先评估的系统虚拟化接口。只有设备确实提供相应 API、服务、VM 类型、调用权限和镜像要求时才可使用。普通 APK 不默认拥有创建/管理 pVM 的权限。
4. 直接 KVM 仅在设备系统明确授权，且 SELinux/DAC/设备节点/ioctl/EL2/Stage-2/GIC/内存管理均得到验证时启用。Root、Shizuku 或存在 /dev/kvm 均不能代替这些条件。
5. 不在 App 中伪造 EL2、Stage-2、GIC、IOMMU/SMMU、pvmfw、DICE、AVB 或 TEE 能力。缺少证据必须报告 UNKNOWN/UNSUPPORTED。
6. 不以关闭 AVB、dm-verity、SELinux、W^X 或伪造 HAL 作为通用启动手段。
7. 每项能力区分设计、实现、构建、自动化测试、开发环境验证和目标真机验证。
8. 同一份虚拟硬件定义必须驱动 VMM 配置、DTB 生成、Guest 驱动预检和兼容性报告。

## 2. 官方 AVF 参考模型

### 2.1 组件职责

| 官方组件 | 职责 | ZeroDroid 对应 |
|---|---|---|
| Android Host / VirtualizationService | 权限、VM 创建管理、资源和生命周期 | AVF Backend Adapter |
| pKVM / ARM64 KVM | EL2 隔离、Stage-2、内存所有权和 pVM 安全 | Hypervisor Adapter |
| crosvm | 用户态 VMM、vCPU、Guest 内存、虚拟设备后端 | 可裁剪的 VMM 候选 |
| pvmfw | pVM 首阶段固件、载荷验证、实例秘密派生 | 仅在真实 AVF pVM 路径使用 |
| Microdroid | 精简 Android Guest、启动管理、载荷运行 | 首个 Guest 参考平台 |
| AuthFS | Host/Guest 文件交换和完整性校验 | 安全文件桥接方案 |
| Binder RPC over vsock | Host/Guest 服务通信 | Guest Agent 与控制面参考 |
| VirtIO | 块、网络、控制台、vsock 等半虚拟设备 | 稳定虚拟设备 ABI |
| AVB / DICE / instance | 启动验证、实例身份、密钥和回滚状态 | Guest 信任链和实例持久化 |

### 2.2 ARM64 执行与内存保护

- Android 用户态通常运行 EL0，Host Linux Kernel 运行 EL1，Hypervisor 位于 EL2。
- pKVM 在设备启动链中由 bootloader 以受信任方式加载/初始化，Host Kernel 随后降权到 EL1。普通应用不能在运行时自行取得 EL2 控制权。
- Stage-1 由 EL1 管理；Stage-2 由 EL2 管理 Guest IPA 到 PA 的访问和权限。pKVM 还保护 Host 自身。
- Guest 内存由 Hypervisor 跟踪 owner/share/lend/relinquish 状态。释放前按安全协议回收和清理；Host 不能继续访问已捐赠且不可访问的页。
- DMA 保护不能只靠 CPU 页表。可访问 Guest 内存的 DMA 设备必须有合格 IOMMU/SMMU 隔离；否则不能宣称具备 pVM 级机密性。
- GICv3、虚拟 Timer、IPI、WFI、PSCI 和 MMIO trap 必须由真实 Hypervisor/VMM 接口支持。DTB 中写入设备节点不会创建设备。

### 2.3 VMM 与设备模型

- KVM 用户态接口包含 system/VM/vCPU/device ioctl；VM FD、vCPU FD、device FD 和 KVM_RUN 构成真实运行闭环。
- vCPU 通常由独立可调度线程驱动；KVM exit 由 VMM 处理 MMIO、设备事件、暂停/关机等状态。
- VirtIO 设备通常由 VMM 提供后端，数据面使用 virtqueue。受保护 Guest 不能默认让不可信 Host 任意读取 Guest 内存；需要按 AVF/pKVM 的共享内存窗口、bounce buffer 和 MMIO guard 约束设计。
- 每个设备定义 transport、地址、IRQ、feature bits、队列大小、共享内存策略、reset/teardown、错误和兼容规则。
- ARM 手机不能套用 PC 的 PCI/ACPI/APIC/HPET/BIOS 默认模型。优先采用明确支持的 VirtIO-MMIO 与 ARM Device Tree；PCI 仅在 Hypervisor、VMM、Guest Kernel 三方均验证支持时开放。

### 2.4 pVM 启动与 Microdroid

1. 设备 bootloader 验证并加载 pvmfw，提供 DICE/CDI 信任材料，并在设备 DT 中保留 pvmfw 内存。
2. Host Kernel 初始化 pKVM，pvmfw 内存从 Host Stage-2 映射中隔离。
3. VMM 创建满足固件 ABI 的 VM/memslot，装载启动材料并启动主 vCPU。
4. pvmfw 验证 AVB 签名载荷，派生实例秘密并构建可信 Guest FDT。
5. Microdroid bootloader 验证 vbmeta、boot、super 等链式分区。
6. Kernel 启动，挂载已验证分区并启动 init。
7. Microdroid Manager 处理 instance 状态、APEX/APK、payload、Guest 生命周期和 Host 通信。
8. AuthFS、Binder RPC over vsock 等提供受控文件/服务通信。

Microdroid 提供精简 Android Native/Bionic、Verified Boot、SELinux、APEX、Binder RPC 等；不提供完整 Android Java Framework、SystemServer、Zygote、图形 UI 或通用 HAL。因此它适合验证 pVM 运行链，不是完整 Android 桌面 Guest 的替代品。

## 3. ZeroDroid 目标架构

1. Android App/UI：VM 管理、ROM 导入、配置、启动报告、控制台、诊断。
2. Android Service/Host Integration：前台服务、生命周期持有、权限检查、FD 管理、通知、系统 API adapter。
3. Kotlin Runtime API：VM API、状态流、错误模型、Host Capability Snapshot、Boot Preflight。
4. JNI/C ABI：稳定句柄、版本化结构、错误码、事件回调边界。
5. Native VM Core：VM 状态机、资源预算、配置验证、生命周期协调和诊断。
6. Hypervisor Backend：AVF、授权 KVM/pKVM，或经单独验证的 OEM Hypervisor。
7. VMM：vCPU 运行循环、Guest memory、KVM/AVF 调用、MMIO/IRQ dispatch、VirtIO 后端。
8. Virtual Hardware ABI：CPU、RAM、GIC、Timer、PSCI、UART、Block、Net、Vsock、Input、GPU 等。
9. Guest Boot Orchestrator：Boot Material Set、Kernel/DTB/Ramdisk/Bootconfig、镜像验证和阶段状态。
10. Image/Storage Engine：OTA、Sparse、GPT、LP、文件系统、AVB、DTB/DTBO、虚拟磁盘与快照。
11. Guest Platform Profiles：Kernel、Vendor/HAL、VINTF、fstab、SELinux、分区和支持功能。
12. Host/Guest Bridge：网络、文件、输入、显示、音频、相机、传感器等最小权限桥接。
13. Security/Observability/Test：密钥、日志脱敏、审计、性能、fuzz、真机兼容矩阵。

模块依赖规则：
- UI 只能调用 Runtime API，不能直接打开 KVM 或操作设备节点。
- Kotlin Runtime 不持有 Activity；VM 生命周期由 Service/明确的持有句柄管理。
- JNI/C ABI 不保存唯一持久状态；句柄具备生命周期校验。
- Image Engine 与 Backend 解耦；解析成功不等于可启动。
- Virtual Hardware Manifest 是 VMM 设备配置、DTB 生成、Guest 驱动检查和兼容性报告的共同来源。
- Guest Profile 不执行 Host 特权操作；Backend 不猜测 Guest 镜像参数。
- 每个 Backend 都有能力证据、支持矩阵和独立端到端测试。
- 设备桥接只暴露显式授权资源，禁止 Guest 获得 Host 任意路径、设备节点或 Binder 服务访问。

## 4. 开发阶段与依赖顺序

阶段可以并行实现独立工作，但不得跳过前置验收；文档完成不算代码完成。

### 阶段 0：官方参考基线与源码审计

工作：
- 固定 Android AVF、Virtualization、pKVM、Microdroid、VirtualizationService、AVB、ACK/GKI、VirtIO、crosvm 的版本和源码 revision。
- 对 manifest 中每个上游项目标记生产代码/参考代码/构建工具/测试工具/排除项。
- 建立 upstream component → ZeroDroid module → build target → runtime dependency 映射。
- 逐 crate 审计 crosvm 的 Android ARM64 可移植性、Bionic、系统调用、seccomp、cgroup、用户切换、FD、设备发现、图形依赖和许可证。
- 不下载/集成与目标无关的 PC 模拟器和 QEMU 代码。

验收：
- 每个采用的上游组件有精确 revision、许可证、用途、改造清单和验证方法。
- 无未知 x86/PCI/ACPI/APIC 生产依赖。
- 明确 AVF API/服务/权限和直接 KVM 的平台差异。

### 阶段 1：Host 能力、权限和 Backend 证据闭环

工作：
- Host 能力分为 silicon、firmware、kernel、device node、system service、permission、backend API、end-to-end 八层。
- AVF Adapter 真实查询服务/API、可用 VM 类型、创建/启动限制、权限失败原因和设备支持信息。
- KVM Adapter 逐级探测架构、节点访问、API version、必要 ioctl/capability、VM create、vCPU/device、内存注册和运行能力；探测受授权且可安全撤销。
- 识别 SELinux/DAC/linker namespace/cgroup/seccomp/FD 继承等 Android 运行约束。
- 将 UNKNOWN、DENIED、UNSUPPORTED、INCOMPATIBLE、RESOURCE_EXHAUSTED、ERROR 分开。
- 后端选择只使用真实证据，不能根据 Android 版本、设备型号或 Root 状态推断。

验收：
- 普通无特权 APK 在不支持设备上稳定返回 unsupported/permission denied，不崩溃、不假报。
- 设备节点可读不等于 VM create；VM create 成功不等于 Guest boot 成功。
- 输出脱敏 Host Capability Report，包含证据来源和采集时间。

### 阶段 2：Android Host Runtime 与 VMM 进程模型

工作：
- 确定 AVF Java/Rust API、系统服务或授权 Native API 的正式接入方式。
- 明确 VMM 由系统 VirtualizationService 托管，或由 ZeroDroid 专用系统集成服务托管。
- 建立 Service 持有 VM Handle、App/UI 可重连观察、进程被杀后按真实后端状态恢复的生命周期。
- 建立 FD ownership/dup/close、异步取消、进程退出、Binder death、超时和资源回收规则。
- 为 VMM 建立 Portable Core、Android Host Adapter、Hypervisor Adapter 边界。
- crosvm 采用逐 crate 最小集合；替换 Android 不兼容依赖，不把服务器 Linux 依赖直接带入 APK。

验收：
- VM 生命周期不依赖 Activity 存活。
- 所有 FD、线程、VM/vCPU 句柄有明确 owner 和释放路径。
- 后端进程崩溃、App 退出、服务重启、用户强制停止均可诊断且不泄漏资源。
- Android ARM64 设备上完成 VMM 最小构建和运行权限验证。

### 阶段 3：VM Core、vCPU 与 Guest Memory

工作：
- 完成 VM create/configure/start/pause/resume/shutdown/force-stop/destroy 状态机。
- 完成 vCPU 创建、初始寄存器/启动参数、线程调度、KVM_RUN/AVF 等待/退出循环。
- 完成 Guest IPA map、memslot、页对齐、溢出检查、内存预算和内存压力降级。
- 明确 protected/unprotected VM 内存分配差异；pVM ownership/share/relinquish 只能经受支持 Hypervisor 接口完成。
- 实现 Guest 内存回收、清零、失败回滚、重复释放和 Host OOM 保护。
- vCPU 亲和性/调度策略仅在 Android 系统允许且测量证明有益时实现。

验收：
- 在授权硬件后端创建并销毁真实 VM/vCPU。
- Guest 内存布局可重复、边界安全、无越界/重复映射。
- 内存不足时启动前拒绝或安全回滚。
- pVM 私有内存不可被 Host VMM 直接读取，必须通过后端/设备实测，不可仅凭单元测试推断。

### 阶段 4：ARM64 虚拟平台和设备 ABI

工作：
- 建立版本化 Virtual Hardware Manifest：CPU feature、RAM、GIC、Timer、PSCI、UART、VirtIO transport、地址/IRQ、设备 feature、共享内存策略。
- CPU 暴露特性必须是 SoC、Hypervisor、VMM 能力交集。
- 实现/接入 GICv3、虚拟 Timer、PSCI、IPI、WFI、MMIO exit 路由。
- 先闭环 UART/console、VirtIO Block、VirtIO Vsock，再实现 Net、Input、GPU、Audio。
- 每个设备定义 feature negotiation、queue、interrupt、reset、shutdown、错误、并发和兼容版本。
- pVM 下 VirtIO 使用明确允许的共享内存窗口/bounce buffer/MMIO guard；不允许 Host 任意映射私有 Guest 页。
- Guest DTB 由同一份 Manifest 生成；校验 CPU/memory/reserved-memory/GIC/timer/PSCI/UART/VirtIO 地址与 IRQ 冲突。

验收：
- VMM 实际创建的设备与 DTB 节点逐项一致。
- Guest Kernel 可识别 CPU/SMP、RAM、GIC、Timer、PSCI 和串口。
- VirtIO Block 可读取测试磁盘，Vsock 可完成双向握手。
- 设备 reset、异常访问、超时和 VM destroy 不遗留 Host 资源。

### 阶段 5：Guest Boot Material Set 与启动编排

工作：
- 将现有 Boot Material Set、boot/vendor_boot/init_boot、Ramdisk、Bootconfig、Kernel、DTB/DTBO、分区和 AVB metadata 解析器接入统一 Boot Orchestrator。
- 从实际 header/version、fstab、LP metadata、分区来源和 Profile 推导材料组合，不按文件名猜测。
- 建立材料来源、哈希、版本、签名、分区关系和依赖边。
- 将现有 FDT v16/v17、DTBO external/local fixup、phandle relocation、fragment merge、symbols merge、序列化流程接入启动材料 pipeline。
- 在最终 Guest FDT 上验证虚拟 CPU、RAM、reserved memory、GIC、Timer、PSCI、UART、VirtIO-MMIO/IRQ 和地址冲突。
- 解析失败、材料缺失、来源不一致时阻止 VM 启动，并输出可定位报告。

验收：
- Guest Profile 可生成完整、可追溯的 Boot Material Set。
- DTB/DTBO 来源和 overlay 顺序确定，最终 FDT 与 VMM 设备配置一致。
- 失败不会产生部分启动材料。
- 真实 dtc/libfdt 互操作测试覆盖关键 FDT/DTBO 行为。

### 阶段 6：AVB、pvmfw、DICE、实例存储与安全链

工作：
- 完成 AVB descriptor/hash/hashtree/chain/property/cmdline 的真实 digest、签名、密钥策略和 rollback index 验证。
- 区分测试信任根和生产信任根；未知密钥/链断裂/回滚拒绝必须显式失败。
- 对接 AVF pvmfw/设备固件真实 ABI，不自行生成冒充设备根信任的秘密。
- 为 VM instance 建立加密持久化、实例身份绑定、启动材料身份记录和更新/回滚策略。
- 研究 DICE/CDI、attestation、sealing key 派生和密钥销毁时机；密钥只经受支持的硬件/系统安全接口管理。
- SELinux、W^X、dm-verity、AuthFS 启用状态纳入 Guest Security Profile。
- 评估 SMMU/IOMMU、DMA 设备复位和所有权切换的隔离完整性。

验收：
- AVB 验证覆盖正例、篡改、错误密钥、链断裂、rollback 和缺失 descriptor。
- 私有实例数据与 VM 身份绑定；未经验证的镜像更新不能静默继承旧实例信任。
- Guest 销毁时私有内存与密钥按后端协议回收。
- 没有 IOMMU/DMA 证据时不声明 pVM 机密性。

### 阶段 7：首个 Guest：Microdroid/最小 ARM64 Linux

工作：
- 选择与目标 Android/AVF 版本一致、可追溯的 Microdroid 或最小 ARM64 Guest 基线。
- 完成 Kernel、bootloader（若需要）、boot/vendor_boot、super、vbmeta、instance、payload 真实组合。
- 实现串口日志、Guest boot stage、Guest Agent、Host 控制通道。
- 先验证 Kernel entry、early console、init、Block、文件系统和 Guest shutdown。
- 实现 vsock、Binder RPC over vsock 或对应 AVF API 通道；文件交换按 AuthFS 或等价完整性协议设计。
- 建立 Guest health、崩溃上报、重启、终止和实例数据管理。

验收：
- 在目标 Android ARM64 设备真实硬件后端启动独立 Guest Kernel。
- Guest init 成功，Block/console/vsock 基础设备可用。
- Host 与 Guest 双向通信，VM 可正常关机和重新启动。
- pVM 启动验证失败必须被固件拒绝，不能用不受信任镜像绕过。

### 阶段 8：Android Guest Framework 与 GSI 平台化

工作：
- 固定首个 Android release/API、ARM64 ABI、Guest Kernel/GKI、Vendor、VINTF、fstab、分区和虚拟设备组合。
- 先让一套匹配的参考 Guest Platform 完整启动，再替换 GSI system。
- 实现 Guest Profile Validator：Kernel config/module/KMI、Boot Header、Ramdisk、DTB、分区、AVB、SELinux、VINTF/HAL、虚拟设备驱动。
- 逐项实现必要 HAL；不支持的 Camera/GPU/Audio/Sensors/Radio/Biometrics/KeyMint 功能明确标记 unavailable，不注册空服务。
- 图形评估 Guest DRM/VirtIO GPU、gralloc/Composer、AHardwareBuffer、DMA-BUF、Fence 和 Host Surface 输出。
- 建立输入、显示、音频、网络、文件、剪贴板等 Host Bridge 的权限和数据通道。

验收：
- Guest 从 Kernel 进入 first-stage init、second-stage init、VINTF/HAL、Zygote、SystemServer、SurfaceFlinger，最终达到预定义 Guest Ready。
- GSI 仅在对应 Guest Platform Profile 验证通过时标记支持。
- 桌面/输入/图形测试必须在真机完成；Kernel Console 不算 Android 桌面启动成功。

### 阶段 9：镜像仓库、OTA、分区、快照与恢复

工作：
- 完成 OTA payload、legacy dat/patch、Sparse、GPT、LP metadata、ext4/EROFS/F2FS、boot chain、AVB 的组合流水线。
- 增加 A/B slot、Virtual A/B snapshot/COW、OTA apply/rollback 和失败恢复。
- 虚拟磁盘使用独立 Guest 数据，不与 Host 原始分区混淆。
- 定义镜像导入原子事务、内容寻址、去重、校验、版本、配额和清理。
- 快照必须由后端能力明确支持；不支持时不提供假快照接口。

验收：
- OTA 输入到 Guest 可启动镜像集的流水线可重复、可校验、可恢复。
- 任意阶段断电/异常后不会留下被误认为有效的半成品镜像。
- 快照/恢复通过崩溃一致性、容量和数据完整性测试。

### 阶段 10：资源自适应、稳定性与安全强化

工作：
- CPU/RAM/存储/温度/电池/后台限制统一预算；动态内存有硬上限和压力降级。
- 设计 VM 并发上限、前后台策略、热管理、OOM 预防、Host 服务优先级。
- 对 VMM 设备输入、镜像解析、DTB、AVB、LP、协议解析实施 fuzz、property tests 和故障注入。
- 增加结构化日志、trace、metrics、启动阶段耗时、vCPU exit 统计、设备 I/O、Guest crash 和 Host pressure。
- 检查密钥/令牌/用户数据日志脱敏，文件路径和 FD 访问控制，IPC 身份和重放保护。

验收：
- 长时间运行、反复启动/停止、内存压力、温控、进程被杀和异常 Guest 均不造成 Host 系统不稳定。
- 安全边界和失败行为有可复现测试；无静默降级和虚假成功状态。

### 阶段 11：设备矩阵、发布和维护

工作：
- 建立按 SoC、Android build fingerprint、Kernel、AVF、pKVM、GIC、IOMMU、GPU、页大小和权限分层的兼容矩阵。
- 区分普通 APK、预装特权 App、系统服务集成和开发者自定义 ROM 的支持范围。
- 每个正式支持组合固定 Guest Profile、镜像哈希、Backend 版本、测试结果和已知限制。
- 建立升级兼容、数据迁移、Guest Profile schema migration、回滚和诊断导出。
- CI 覆盖 Linux 构建/单元测试/ABI/schema/fuzz；设备实验室覆盖 Android 真机虚拟化与启动。

验收：
- 发布说明只声明通过验证的设备/Guest 组合。
- 每次发布保留测试报告、源码 revision、签名材料来源和兼容性矩阵。
- CI 编译通过不能替代真机硬件虚拟化验证。

## 5. 首个可交付里程碑

MVP-1：在至少一台明确支持且授权 AVF/KVM 的 ARM64 Android 设备上，启动独立 Guest Kernel，并通过 VirtIO Block + Console + Vsock 完成最小闭环。

交付：
- Host Capability Report；
- 真实 Backend Adapter；
- VM/vCPU/memory 生命周期；
- ARM64 Virtual Hardware Manifest；
- 与 Manifest 同源的 DTB；
- Guest Kernel/Boot Material Set；
- Console、Block、Vsock；
- 启动阶段事件、错误报告和资源清理；
- 真机日志、设备构建指纹和可复现测试记录。

MVP-1 不声称完整 Android 桌面、通用 GSI、GPU 加速、pVM 安全认证或任意 ROM 兼容。完成后再推进 Microdroid pVM 验证和完整 Android Guest。

## 6. 已有实现与当前缺口

### 6.1 可复用的已有基础

根据当前 PR 分支源码和文档：
- Kotlin Runtime API、VM 模型、能力门控已有基础实现。
- Native C ABI、JNI 和 /dev/kvm 节点/API 版本探测已有基础实现；它不是 VM create/run backend。
- Rust image core 已有多类镜像/分区解析基础。
- FDT v16/v17、DTBO external/local fixup、phandle 声明和引用 relocation、overlay fragment merge、symbols merge、owned-tree canonicalization、FDT v17 serialization 和原始 DTBO sequence pipeline 已实现并有测试。
- DTC 真实 fixture 已纳入 Rust CI。
- Boot Material Set 和 Guest Boot Profile 已有规范/部分解析基础。

### 6.2 尚未形成生产闭环的核心工作

- 生产 AVF Backend 和授权 KVM/pKVM Backend。
- VMM 进程、真实 VM/vCPU 创建与运行、退出处理。
- Guest memory ownership 与 pVM 私有内存生命周期。
- GIC/Timer/PSCI/IRQ/MMIO 的实际后端接入。
- 可运行的 VirtIO Block/Console/Vsock/Net 设备。
- Virtual Hardware Manifest 到 VMM 和 DTB 的同源生成与验证。
- Boot Material Set 到真实 Backend 启动参数/FD 的接线。
- 完整 AVB cryptographic verification、可信根、rollback persistence。
- pvmfw/DICE/instance image/attestation 的真实系统集成。
- 可在真机启动的 Guest Kernel、Microdroid 和完整 Android Guest。
- Guest VINTF/HAL、图形、音频、输入和 Host Bridge 的端到端验证。
- 真机性能、温度、内存压力和多 VM 稳定性验证。

## 7. CI 与验证体系

1. Static/Schema：JSON Schema、Manifest、ABI layout、version compatibility、license/source manifest。
2. Rust：unit/property/fuzz、parser boundary、overflow、malformed input、DTC/libfdt differential fixtures。
3. Native：C ABI layout、handle lifecycle、thread safety、FD ownership、sanitizer、CMake。
4. Android：Kotlin unit/instrumentation、JNI load、Service lifecycle、permission denial、Binder death、FD transfer。
5. VMM integration：VM create/destroy、vCPU run/exit、memory map、MMIO、IRQ、VirtIO queue、reset。
6. Guest boot：Kernel entry、early console、init、root mount、Guest Agent、shutdown/reboot。
7. Security：AVB tamper、rollback、instance corruption、memory isolation、DMA/IOMMU、AuthFS integrity、SELinux denial。
8. Device lab：真实 ARM64 设备、实际系统构建、AVF/KVM 授权、温控/内存压力、长稳测试。

测试结果必须带设备/系统/Kernel/Backend/Guest Profile revision。没有设备证据的能力标记为未验证。

## 8. 关键风险

| 风险 | 影响 | 策略 |
|---|---|---|
| 普通 APK 无 AVF/KVM 特权 | 无法创建 VM | 确定系统集成/特权授权方案；不伪造权限 |
| OEM 未启用 pKVM/EL2 | 无法实现受保护 VM | 设备分级；非 pVM 后端不得冒称 pVM |
| IOMMU/SMMU 不满足隔离要求 | DMA 可绕过 CPU 隔离 | 禁止声明机密性；限制设备共享 |
| crosvm 依赖 Linux 系统服务 | Android 运行失败 | 逐 crate 审计并实现 Android Adapter |
| GSI 与 Vendor/HAL 不匹配 | Framework 无法启动 | 固定 Guest Platform Profile；先参考系统再替换 GSI |
| Guest DTB 与 VMM 设备不一致 | Kernel 启动/驱动失败 | Manifest 单一事实来源 + 自动一致性测试 |
| AVB/实例信任链不完整 | 启动安全失效 | 完整 crypto/trust/rollback 实现前禁止生产标记 |
| 手机内存/热预算不足 | Host OOM/过热 | 动态预算、硬上限、压力停止和长稳测试 |

## 9. 优先级总表

| 优先级 | 工作 | 依赖 | 结果 |
|---|---|---|---|
| P0 | 上游版本/AVF API/设备权限审计 | 当前源码 | 可执行平台基线 |
| P0 | AVF/KVM Backend 能力和授权闭环 | P0 审计 | 真实可创建 VM 的后端 |
| P0 | VMM Android ARM64 最小进程模型 | Backend 设计 | 可运行 VMM |
| P0 | VM/vCPU/Guest memory 生命周期 | VMM/Backend | 可执行 Guest CPU |
| P0 | GIC/Timer/PSCI/Console/Block/Vsock | VM Core | 最小虚拟硬件 |
| P0 | Manifest → VMM + DTB 同源生成 | 虚拟硬件 ABI | 可启动 Guest 平台描述 |
| P0 | Boot Material Set 接线和 Guest Kernel 启动 | VM/设备/镜像 | MVP-1 |
| P1 | AVB crypto、instance、pvmfw/DICE | 启动闭环 | 可信启动 |
| P1 | Microdroid/Guest Agent/AuthFS/RPC | MVP-1 | 可管理 Guest |
| P1 | Guest Android Framework/VINTF/HAL | Guest 基线 | Android 系统启动 |
| P2 | GPU/Display/Input/Audio/Network Bridge | Guest Framework | 可交互桌面 |
| P2 | OTA/A-B/COW/snapshot/恢复 | 镜像闭环 | 可维护系统 |
| P2 | 多设备兼容矩阵/性能与安全审计 | 端到端能力 | 可发布产品 |

## 10. 官方参考文档

- [Android Virtualization Framework overview](https://source.android.com/docs/core/virtualization)
- [Why AVF?](https://source.android.com/docs/core/virtualization/whyavf)
- [AVF architecture](https://source.android.com/docs/core/virtualization/architecture)
- [Microdroid](https://source.android.com/docs/core/virtualization/microdroid)
- [VirtualizationService](https://source.android.com/docs/core/virtualization/virtualization-service)
- [AVF Security](https://source.android.com/docs/core/virtualization/security)
- [AVF use cases](https://source.android.com/docs/core/virtualization/usecases)
- [AOSP Virtualization source](https://android.googlesource.com/platform/packages/modules/Virtualization/)

官方文档持续更新。每次引入新的 AVF API、Kernel ABI、pvmfw 或 Microdroid 行为时，必须重新核对目标 Android release 对应的源码和设备实现。
