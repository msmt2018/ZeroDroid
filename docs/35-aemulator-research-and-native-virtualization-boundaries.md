# 35. AEmulator 架构研究、兼容性借鉴与 ZeroDroid 原生虚拟化边界

## 1. 文档目的与使用边界

本文记录对 `uxazu/AEmulator` 公开源码的架构研究结果，为 ZeroDroid 的独立实现提供技术参考。AEmulator 是研究样本，不是 ZeroDroid 的生产运行后端，也不是可直接移植的基础工程。

- 允许借鉴：问题拆分方式、兼容性测试思路、ROM 格式识别策略、系统调用语义分析、Binder/GPU/HAL 边界划分、故障诊断方法。
- 必须独立实现：ZeroDroid 的 VM 生命周期、VMM/Backend、Guest 内核启动链、虚拟设备、设备访问策略、Binder 设备路径、图形与 HAL 桥接、存储和恢复机制。
- 不得把 AEmulator 的用户态进程环境包装成完整系统 VM。
- 不复制其源文件、算法实现或非必要代码片段；若未来确有必要采用任何上游代码，必须先逐文件核对 GPL-3.0 义务、依赖许可证、NOTICE、派生作品分发义务和用户提供 ROM/固件的独立版权。默认采取 clean-room 重实现。

## 2. 已审阅的关键实现

研究对象为 AEmulator 的公开 main 分支，重点文件：

- `native/guest64/21/run64.c`
- `native/guest64/37/run64.c`
- `native/guest64/37/binderd.c`
- `app/src/main/java/app/aemu/core/InitPlan.kt`
- `app/src/main/java/app/aemu/core/GuestVm.kt`
- `app/src/main/java/app/aemu/importer/Importer.kt`
- `native/glbridge/guest.c`、`native/glbridge/host.c`（后续仍需逐函数审计）

本节结论以已读取的代码为依据，不代表对 AEmulator 全仓库完成安全审计。

### 2.1 ARM64 用户态执行与系统调用兼容

`run64.c` 的注释和实现表明：ARM64 Guest ELF 由手机 ARM64 CPU 原生执行，Guest 保留自己的 linker/bionic；seccomp filter 将需要改变 Guest 视图的系统调用导向 SIGSYS handler。路径、身份、部分 ioctl、内存信息和其他兼容语义由该层重定向或合成，未被拦截的系统调用仍进入 Host Linux Kernel。

可借鉴：
- 以系统调用语义为单位建立兼容矩阵，而不是只看 Android API Level。
- 将 Guest-visible 路径、身份、属性、内存视图和设备节点作为独立适配域。
- 为每个重定向系统调用建立输入边界、错误语义、权限策略和测试向量。
- 针对不同 Android 版本维护明确的行为差异，不以一个通用分支覆盖全部 ROM。

不能照搬为 ZeroDroid 主架构：
- 这种机制运行在宿主进程和宿主 Kernel 之上，Guest 没有独立内核、独立 EL1 内核态、Stage-2 地址空间和独立中断控制器。
- seccomp/SIGSYS 不是 Hypervisor，不提供 VM 级内存隔离或 Guest Kernel 隔离。
- 用户态路径重定向和伪造 /proc、/sys 视图不能替代 Guest 自己的 VFS、procfs、sysfs、SELinux 和驱动。
- 它可以成为独立的 ROM 兼容研究材料，但不能成为 ZeroDroid 的系统执行路径。

### 2.2 Binder 用户态协议实现

`binderd.c` 声明自身为用户态 Binder driver replacement。每个 Guest 线程通过独立 Unix `SOCK_SEQPACKET` 连接，daemon 使用单线程 epoll loop 管理进程、线程、Binder node、reference、transaction stack、death notification、looper 和 buffer 生命周期，并在用户态转换 Binder 对象和 FD。

可借鉴：
- Binder ABI 需要覆盖对象类型、引用计数、事务同步/异步、线程池/looper、死亡通知、FD 传递和 buffer 生命周期。
- 将 Binder 的协议测试拆成进程、线程、节点、引用、事务、错误恢复和并发场景。
- 为每一项 Binder 兼容能力标记已实现、部分实现、未实现和版本差异。

ZeroDroid 的目标实现：
- 首选 Guest Kernel 自己的 Binder 驱动及匹配的 Android Binder UAPI/驱动版本。
- Guest servicemanager、hwservicemanager、vndservicemanager 和 framework 使用 Guest 内核 Binder 设备。
- Host/Guest Binder 不应无条件共享对象命名空间或 FD；跨边界调用必须经过显式桥接协议、身份映射、权限校验和资源上限。
- 用户态 Binder 兼容层只允许作为受限兼容模式或测试工具，不能伪装成 Guest Kernel Binder。

### 2.3 init 与系统服务启动

`InitPlan.kt` 读取 ROM 中的 init rc 文件，解析 service/export/socket/user/group/class 等有限字段，并使用维护的启动顺序启动选定服务。其注释明确指出，部分厂商服务不启动，因为由 Host 代替或它们会操作硬件。

可借鉴：
- ROM 导入时建立 init rc、service、socket、property、APEX、VINTF、HAL service 的依赖索引。
- 在启动前进行服务依赖检查，并输出缺失二进制、属性、库、socket 和 HAL 的诊断。
- 使用真实 ROM 中的 service 声明来发现厂商差异，而不是只维护固定 AOSP 服务名单。

ZeroDroid 不应使用固定列表替代 Guest init：
- Guest Kernel 启动后应执行 Guest ramdisk 中匹配版本的 Android `/init`。
- 由 Guest init 处理 import、action trigger、property trigger、mount、mkdir、restorecon、service class、oneshot、critical、restart、socket、interface、namespace、capability、UID/GID、SELinux domain 等真实语义。
- VMM/设备模型应提供 Guest 所需的虚拟设备和启动参数；Guest init 不应由 Host Kotlin 代码模拟。
- ROM 兼容层负责验证和诊断 init 依赖，不应接管 Guest init 的职责。

### 2.4 ROM 与 OTA 导入

`Importer.kt` 显示了多层容器探测、嵌套归档、ZIP/7z/TAR、压缩流、Android boot image、Sparse、EXT4、EROFS、super image、payload 和厂商固件格式等导入路线。

可借鉴：
- 先识别容器，再识别其内部 payload；使用可寻址源抽象支持 seekable 与 non-seekable 输入。
- 把格式识别、解压、分区提取、文件系统读取、元数据分析、Guest Profile 生成和最终发布拆成流水线阶段。
- 为 ROM 家族维护可追溯的格式探测结果和兼容性诊断。

ZeroDroid 的增强要求：
- 所有解压、递归嵌套、条目数量、单文件大小、总展开大小、压缩比、路径长度、符号链接深度、解析时间和临时空间都必须有硬上限。
- 采用流式复制、随机访问索引、分块解析和有界缓存；禁止无上限 readAllBytes/整镜像驻留 RAM。
- 路径防护必须同时考虑词法路径穿越、符号链接逃逸、硬链接、重复条目、大小写冲突和目录覆盖。
- 不可信镜像解析器使用 fuzz、属性测试、损坏样本和差分测试。
- 只有通过完整校验的分区和元数据才能进入 VM 镜像仓库；导入失败必须原子清理临时对象。

### 2.5 GPU、图形和设备桥接

AEmulator 的 GL bridge 方向说明 Guest EGL/GLES 调用可以通过 Host/Guest 协议桥接到手机图形栈，但这不是自动获得 GPU 直通，也不等于零拷贝。需要进一步逐函数核对 GL 命令、共享 buffer、EGLImage、同步 fence、gralloc 和生命周期实现。

ZeroDroid 的优先顺序：
1. 先探测 Host 实际支持的图形 API、扩展、buffer import/export、fence 和 present 路径。
2. 若平台支持受控 GPU 虚拟化后端，优先使用成熟的硬件加速虚拟 GPU 协议/实现。
3. 对 AHardwareBuffer、DMA-BUF、EGL/Vulkan 外部内存和同步 fence 建立端到端能力测试。
4. 不能安全直通的 GPU 资源由 Host 图形服务代理；软件渲染仅作为明确标注的兼容降级，不声称等同真实 GPU。
5. 任何 Guest buffer 都必须有 VM 归属、尺寸/格式/usage 校验、同步和释放规则。

## 3. ZeroDroid 的硬件真实性分级

“真实硬件级”不能理解为 Guest 可以不受控制地直接访问手机全部硬件。手机通常只有一个物理 SoC、一个 GPU 和一套存储控制器，Host Android 仍需要管理电源、显示、热管理、安全和用户数据。正确目标是最大化真实硬件执行，同时保留必要的隔离与共享设备管理。

### H0：真实硬件执行（目标主路径）

- ARM64 Guest 指令由物理 CPU 核执行，不进行指令翻译。
- 在硬件虚拟化可用时，Guest Kernel 运行于虚拟化提供的非安全 EL1 环境，由 EL2 Hypervisor 管理 VM。
- Guest 内存通过 Stage-2 页表映射到 Host 分配的物理页；Guest 获得独立 IPA 空间，但不能任意访问 Host 物理内存。
- vCPU、虚拟定时器、中断和 GIC 状态由硬件虚拟化与 VMM 协作管理。
- Guest Kernel、init、Binder driver、Framework、system_server 和应用进程均在 Guest 内部运行。

这属于硬件辅助虚拟化，不是软件 CPU 模拟。Guest 的 CPU 型号/拓扑视图仍需由 VMM 定义并受 Host 实际可用 CPU、调度和安全限制。

### H1：受控硬件设备直通

仅在 SoC、IOMMU/SMMU、驱动、固件、Host Kernel 和 VMM 全部支持且设备可安全独占时使用：
- 设备必须能从 Host 驱动安全解绑或通过受支持的虚拟化接口分配。
- 必须具备 DMA 隔离、IOMMU 映射、复位、错误恢复和生命周期管理。
- 不允许 Guest DMA 访问 Host 任意内存。
- 对手机共享 GPU、显示控制器、UFS/eMMC、ISP、音频 codec 等资源，不能假设可独占直通。

### H2：硬件加速的虚拟设备

当设备共享或不支持直通时，Host 仍可用真实硬件执行工作：
- 虚拟 GPU 命令由 Host GPU 驱动提交到真实 GPU。
- 虚拟块设备由 Host 存储栈访问 VM 私有磁盘文件/卷。
- 虚拟网络通过 Host 网络栈、受控 TAP/vsock/数据通道工作。
- 相机、音频、传感器通过明确授权的 Host Bridge 使用真实传感器和设备。

这不是软件模拟硬件，而是虚拟设备接口 + 真实 Host 硬件后端。需要如实标注额外复制、调度和同步开销。

### H3：软件模拟或兼容降级

只用于测试、无硬件加速路径或特定兼容问题。必须显示能力等级和性能限制，不得静默把它作为生产主路径。ZeroDroid 不采用 QEMU/TCG 作为生产 CPU 执行后端。

## 4. 硬件配置参数：真实资源与 Guest 设备身份分离

必须区分三种数据，不能把它们混成一个“硬件配置”：

1. **Host 实测事实**：SoC/CPU feature、可用 vCPU、RAM 压力、GPU 扩展、存储余量、温度、功耗、设备节点、Hypervisor 服务和权限。
2. **Guest 分配资源**：vCPU 数、Guest RAM、虚拟磁盘容量、网络带宽预算、图形 buffer 限额。这些是资源分配和调度契约，不是虚构的物理硬件。
3. **Guest 设备身份**：设备树、CPU feature mask、屏幕密度/分辨率、GPU capabilities、传感器清单、属性、VINTF、HAL manifest、产品配置和固件身份。它们必须彼此一致，并且不能宣称 Guest 拥有实际未提供的能力。

推荐策略：
- 默认使用 Host 能力的安全交集，而不是任意伪造旗舰设备身份。
- CPU feature 只能暴露 Host 与 Hypervisor 都支持且 Guest ABI 允许的交集。
- 内存大小是明确分配的 Guest RAM，必须有硬上限和 Host 内存压力保护。
- GPU、相机、音频、传感器能力按真实桥接/直通状态生成，未实现的 HAL 不得宣告存在。
- Guest profile 可以选择兼容设备身份，但身份配置必须通过一致性校验；不一致时阻止启动或明确降级。
- 不向 Guest 暴露 Host 的敏感硬件序列号、密钥、设备标识或其他 VM 的资源状态。

## 5. ZeroDroid 后端能力契约

每个后端都必须分别报告以下证据，不能用一个布尔值表示“支持虚拟化”：

- silicon：CPU 是否实现 ARM virtualization extensions。
- firmware：EL2/安全固件是否将虚拟化能力交给 Host。
- kernel：Host Kernel 是否启用并暴露对应 hypervisor/KVM/pKVM 接口。
- service：AVF/VirtualizationService 或 OEM hypervisor service 是否可用。
- permission：当前应用是否获准调用相应接口。
- vmm：VMM 是否能创建、配置和运行 VM。
- devices：中断控制器、内存、块设备、网络、vsock、GPU 等设备是否可用。
- guest boot：指定 Kernel、DTB、ramdisk、AVB 和 Guest Profile 是否实际启动。
- lifecycle：start/stop/reboot/crash recovery 是否完成端到端验证。

能力状态应至少包括 unsupported、unknown、detected、authorized、configured、runtime-tested、device-validated。只有端到端测试成功才能标记为 device-validated。

## 6. AEmulator 研究转化为 ZeroDroid 工作项

| 研究领域 | ZeroDroid 自研目标 | 验收重点 |
|---|---|---|
| syscall compatibility | Guest Kernel + VMM ABI + 有限 Host Bridge | Guest 系统调用由 Guest Kernel 处理，跨边界操作有权限与资源约束 |
| Binder | Guest Kernel Binder 驱动及版本匹配 | Binder CTS/驱动协议测试、FD/死亡通知/线程语义 |
| init | 原生 Guest init 启动 | init rc/action/property/service/socket/SELinux 生命周期 |
| ROM importer | 独立 Rust/C++ 有界解析流水线 | fuzz、超限拒绝、路径隔离、断点清理 |
| GPU bridge | 虚拟 GPU + Host GPU 硬件执行 | buffer/fence/render/present 端到端 |
| HAL | Guest HAL + 显式 Host 设备代理 | VINTF 匹配、权限、设备生命周期和故障隔离 |
| resource view | Host 资源预算映射到 Guest | 内存/CPU 视图一致、压力回收和热管理 |

## 7. 当前实现状态与禁止误报

本文件是架构研究与实现要求，不代表相关功能已经开发完成。

当前仓库中已有的虚拟 Fastboot、GPT/LP 映射、块 I/O、Sparse COW、Journal/Mutation Replay 属于平台无关核心和协议层；它们不等于已实现 Android 文件后端、真实 VMM、VirtIO 设备、Guest DMA、GIC、中断注入、Guest Kernel 启动或真机硬件直通。

每个功能的状态必须分别记录：
- design：架构/接口定义；
- implemented：源代码存在；
- unit-tested：单元测试通过；
- ci-validated：目标提交的 CI 通过；
- integration-tested：模块集成测试通过；
- device-validated：指定真机、系统版本、内核和权限下端到端验证通过。

不得将其中任一低级状态描述为已实现完整虚拟机。

## 8. 下一阶段工程顺序

1. 完成 AEmulator GL bridge、GuestVm、GuestRunner、TreeFixer 和 importer 的逐函数审计；记录可借鉴点、不可借鉴点和安全边界。
2. 建立 ZeroDroid 的 Host Hardware Capability Evidence 与 Backend Capability Contract，连接现有 AVF 探测实现。
3. 明确真实硬件执行后端：AVF/pKVM 或系统授权 KVM；逐设备验证 VMM 可用性，不默认任何普通 APK 权限。
4. 实现 Guest 内存、vCPU、虚拟中断控制器和 Guest Kernel boot 的最小端到端路径。
5. 再接入虚拟块设备与现有分区/COW/journal 层，确保写入、flush、恢复和刷机事务真正持久化。
6. 后续实现 Guest display/GPU、input、audio、network、camera、sensor 等设备路径，并为每种路径标记 H1/H2/H3 等级。

## 9. 参考源码与许可提醒

研究来源：
- https://github.com/uxazu/AEmulator
- https://github.com/uxazu/AEmulator/blob/main/native/guest64/37/run64.c
- https://github.com/uxazu/AEmulator/blob/main/native/guest64/37/binderd.c
- https://github.com/uxazu/AEmulator/blob/main/app/src/main/java/app/aemu/core/InitPlan.kt
- https://github.com/uxazu/AEmulator/blob/main/app/src/main/java/app/aemu/importer/Importer.kt

AEmulator 仓库标注 GPL-3.0。该许可证不自动授予第三方 ROM、厂商固件、预编译 blob 或品牌商标的再分发权。ZeroDroid 应维护独立的第三方依赖清单、许可证清单和 ROM 来源/版权记录。


## 10. 第二轮源码审计：GL Bridge、GuestRunner 与 TreeFixer

### 10.1 GL Bridge 的真实执行路径

已读取 `native/glbridge/guest.c` 与 `native/glbridge/host.c` 的实现开头及核心结构：

- Guest 侧为 GLES API 建立代理入口，按绘制线程维护命令流；每个绘制线程使用 Unix socket 连接 Host。
- Host 侧为 Guest 绘制线程创建真实 EGL/GLES context，并在手机 GPU 上重放命令流。
- Guest window 映射为 Host context 中的 framebuffer object；swap 路径将完成帧复制/回传到应用窗口。
- Guest EGL display/config 与部分 context 路径保留 SwiftShader；是否走真实 Host GPU 由 context policy 决定。
- Host 实现维护 Guest/Host GL object 映射、context share group、surface、buffer ring、fence 等状态。

这说明 AEmulator 采用的是“Guest API 代理 + Host GPU 命令执行”的硬件加速桥接，不是 Guest 获得 GPU 设备直通，也不能据此宣称零拷贝。命令编码、socket 往返、状态同步、buffer 上传/回读和 frame present 都可能产生额外开销。

ZeroDroid 的借鉴方向：
- 明确区分 GPU 直通、虚拟 GPU 命令转发、Host GPU 代理和软件渲染。
- 为 GL/Vulkan 协议定义版本协商、命令长度上限、资源 ID 归属、对象生命周期、同步 fence 和错误恢复。
- 重点验证 buffer 是否能通过 AHardwareBuffer/DMA-BUF 等受支持路径共享，减少 CPU copy；不能支持时必须如实选择复制路径。
- Host GPU context 不得跨 VM 混用对象或同步状态。
- 不直接复用 AEmulator GL 命令协议或源代码；优先评估 Android/virtio-gpu/gfxstream/Venus 等公开协议与可维护实现，并验证 Android ARM64 设备侧可用性。

### 10.2 GuestRunner：执行器选择及其边界

`GuestRunner.kt` 根据 ROM API/engine 选择不同执行路径：
- 新版 ARM64 路径运行原生 ARM64 runner，并通过环境变量传递 ROM root、属性区、Binder socket、输入 socket 和 GPU bridge endpoint。
- 旧版 Guest 32-bit 路径使用 QEMU user-mode，部分设备在支持 AArch32 时直接运行 32-bit binary。
- Guest 运行环境由宿主 ProcessBuilder 创建，路径和环境由 Host 管理。

该路径再次确认它不是独立 Guest Kernel/VMM。ZeroDroid 应由 VMM 创建 Guest vCPU 并加载 Guest Kernel/DTB/ramdisk，不能把 Guest ELF ProcessBuilder 执行流程升级包装成系统 VM。

### 10.3 TreeFixer：ROM 修补策略

`TreeFixer.kt` 的 `sanitize()` 会把一批厂商 HAL、GPU/EGL 驱动和 OMX 模块移入 `system/.aemu-parked`，并保留可恢复副本；`fixup()` 则安装 engine 文件、保护预编译 OAT、调整主题/厂商服务、创建设备节点和系统目录等。

可借鉴：
- ROM 修改必须可追踪、可逆，记录原始文件哈希、变更原因、变更工具版本和恢复路径。
- 对 HAL/驱动冲突进行分类诊断，不能简单按文件名就认定某个模块不可用。
- 建立 Vendor HAL、VINTF、GPU、音频、相机和传感器依赖矩阵。

不能直接用于 ZeroDroid 的原因：
- 真正的 Guest Kernel/Guest HAL 需要加载与 Guest Kernel、DTB、VINTF 和设备模型匹配的驱动/模块。
- 批量停放 Vendor HAL 可能使依赖这些服务的 system_server、surfaceflinger、media、camera 或厂商 Framework 失去必要能力。
- ZeroDroid 应优先保留原始 ROM 镜像，只在明确的兼容转换步骤中生成独立派生镜像；所有修改采用事务式、可回滚的镜像变换，不原地破坏用户导入的 ROM。

## 11. 本轮审计结论更新

第二轮审计确认 AEmulator 的 GPU 路径是 Host GPU 代理执行，GuestRunner 是 Host 进程启动 Guest 用户态程序，TreeFixer 则主动调整 ROM 文件树以适配“无独立 Guest Kernel/硬件”的运行环境。这些技术对 ZeroDroid 有研究价值，但与我们要实现的系统级 VM 在权限模型、内核边界、驱动加载、设备所有权和启动链上存在根本差异。

尚未完成的审计包括 GL bridge 的全部命令分发/同步实现、GuestVm 的完整启动和退出流程、Importer 中每一种格式解析器的资源边界，以及 AEmulator 第三方二进制和许可证逐项核验。不能把本轮结果称为完整安全审计。
