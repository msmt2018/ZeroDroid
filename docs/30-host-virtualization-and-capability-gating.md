# 30. Host 虚拟化能力门控、权限与后端选择

## 1. 目标

ZeroDroid 必须依据真实 Host 设备和系统环境选择后端。SoC 理论上支持 ARM 虚拟化，不代表 Android 内核、EL2 所有权、设备节点、系统服务和应用权限允许第三方 App 创建 VM。

## 2. 分层能力探测

Native Core 通过 C ABI/JNI 分别报告 Host 架构、/dev/kvm 打开结果、KVM API 版本，以及一次短暂的 VM 创建授权探测。探测过程调用 KVM_CREATE_VM 创建一个空 VM fd，并立即关闭该 fd；它不创建 vCPU、不注册 Guest 内存、不加载镜像、不启动 Guest。这样可以把“设备节点可访问”和“当前进程有权创建 VM”区分开。

KVM device/API 状态与 VM_CREATE 状态独立报告，均保留 AVAILABLE、UNAVAILABLE、ACCESS_DENIED、INCOMPATIBLE、ERROR 等结果。打开设备失败时保留 errno；VM_CREATE 失败时单独保留 errno。API 版本不兼容时不会继续尝试创建 VM。该探测不是无副作用的纯查询：它会短暂创建并销毁一个空 VM，因此必须只在用户主动请求能力检查或 Runtime preflight 时执行，避免高频轮询。

即使 VM_CREATE 成功，也只证明当前进程可创建空 VM。它不证明 vCPU 创建、内存注册、EL2/Stage-2、GIC、IOMMU/SMMU、Guest 启动或端到端虚拟化可用。AVF 探测仍须独立验证 API、VirtualizationService、应用授权及目标 VM 类型。

每项能力分别记录 silicon、firmware、kernel、device node、service、app permission、backend API 和 end-to-end 状态。CPU feature 检测结果不能直接当作 KVM/AVF 可用结论。

## 3. Backend 选择

Kotlin runtime API 中的 HostCapabilityGate 是保守决策器，输入必须是实际采集的证据，而不是设备型号推断。AVF 必须同时具备 API、服务、应用授权及目标 VM 类型支持；直接 KVM 必须同时具备 AArch64、KVM API v12、VM 创建授权、EL2、Stage-2 和 ARM GIC 证据。未知、拒绝、错误或不兼容均视为 blocker。决策器只在完整满足条件的硬件后端中按偏好选择；没有满足条件的后端时返回 unsupported，不存在 CPU 模拟或容器回退。

### 3.1 能力证据与决策边界

- CapabilityFact.UNKNOWN 不等于不可用，也绝不等于可用；必须继续探测或报告未知。
- /dev/kvm 可打开且 API 版本正确，只能填充 KVM device/API 证据。
- VM_CREATE 探测成功可填充 vmCreationAuthorized=AVAILABLE；它不能填充 EL2、Stage-2 或 GIC 证据。
- 应用是否被允许创建 VM、EL2 是否由可用 Hypervisor 管理、Stage-2 与 GIC 能力，必须由受支持的系统接口或授权后端证明。
- AVF service/API/授权状态不能通过检查包名或 Android 版本推断。

AVF：检测设备实际提供的 API、VirtualizationService、权限和支持的 VM 类型。AVF API/权限受 Android 版本、系统镜像、设备实现、应用预装/特权授权和具体 API 约束，不能简单规定 Android 13+ 所有手机可用。

直接 KVM/VMM：需要 Host Kernel 暴露 KVM 接口、权限、EL2/虚拟化控制权和兼容的 VMM/设备后端。/dev/kvm 是否存在只是检查项，不代表 App 可访问或创建所需 VM。crosvm 仅是 VMM 候选，不是 Hypervisor；可在 Android ARM64 KVM 上运行也必须通过系统权限和设备闭环验证。

厂商 Hypervisor：Gunyah、GenieZone 等接口受 SoC、厂商、内核和系统集成约束，不能当作 Android 通用标准，需独立适配、授权和真机验证。

软件 CPU 仿真（包括 QEMU TCG）不属于 ZeroDroid 运行后端，也不得作为完整 Guest 的自动 fallback。没有可用硬件虚拟化时，应明确报告设备不支持完整 VM。

## 3.2 能力探测执行框架

AndroidHostCapabilityDiscovery 已将 AndroidHostArchitectureProbe、AndroidAvfCapabilityProbe、AndroidKvmCapabilityProbe 接入 HostCapabilityDiscovery。Android AVF probe 使用官方 system feature 字符串和 MANAGE_VIRTUAL_MACHINE 权限检查；由于 VirtualMachineManager 属于受限 @SystemApi，普通 public SDK adapter 不通过反射或隐藏 Binder 查询服务状态及 VM 类型，因此 serviceAvailable 保持 UNKNOWN、supportedVmTypes 为空。Android KVM probe 将 Native C ABI 的 device/API 与临时 VM_CREATE 结果映射为独立证据；EL2、Stage-2、GIC 仍保持 UNKNOWN。HostCapabilityDiscovery 将普通异常和 LinkageError 转成 ERROR 事实并附带来源与异常类型；协程取消继续向上传播。报告同时返回原始证据、后端决策和探测失败记录，便于 UI/日志解释阻断原因。

## 4. Root、Shizuku 与系统权限

Shizuku、su、KernelSU、Magisk 不会自动提供 Hypervisor、EL2 或 AVF 管理权限。Root 不能凭空启用缺失硬件能力、内核配置或安全固件接口。刷入修改内核是独立实验路径，带来启动、安全和 OTA 风险，不能作为普通用户前提。

## 5. 资源预算

CPU、RAM、GPU、存储和并发 VM 数量均按 Host 实际探测与实时压力计算。考虑 Android 前后台限制、内存压力、温度、电池、存储余量、GPU backend 和用户负载。不能规定所有设备必须 8GB RAM，也不能固定 Guest 分配 4–6GB。

## 6. GPU 能力门控

Vulkan 版本/扩展存在并不足以证明 gfxstream/Venus、AHardwareBuffer、DMA-BUF、同步 Fence 和零拷贝路径可用。必须逐项探测并执行 buffer import/export、render、present 和同步测试。不可用时选择可验证的软件渲染或禁用对应能力，并明确性能等级。

## 7. Host Capability Manifest

记录 host_fingerprint、android_api、kernel_release、abi、cpu_features、virtualization_extensions、avf_api、service_status、device_nodes、permission_status、backend_candidates、gpu_extensions、memory_budget、storage_budget、thermal_state、verified_features 和 probe_timestamp。敏感标识仅用于本地诊断，导出前脱敏。

## 8. Android ARM64 Host 移植门控

SoC 是 ARM64 并不代表上游 VMM 可直接在 Android 上运行。必须检查 Bionic/NDK、Linux 用户态服务依赖、SELinux、linker namespace、cgroup/seccomp、设备节点和厂商 Hypervisor ABI；PC x86/PCI/ACPI/APIC/BIOS 路径必须移除或隔离。VMM 需分为 Portable Core、Android Host Adapter 和 Hypervisor Adapter。具体审计和测试标准见 [Android ARM64 宿主移植规范](37-android-arm64-host-porting-and-adaptation.md)。

## 9. 验收

探测结果区分理论能力与端到端验证；每个 Backend 有 capability contract 和明确错误码；无权限或后端不可用时不崩溃、不假报成功；资源配置动态且有硬上限；每种后端记录具体真机型号、系统版本和构建指纹测试结果。
