# 30. Host 虚拟化能力门控、权限与后端选择

## 1. 目标

ZeroDroid 必须依据真实 Host 设备和系统环境选择后端。SoC 理论上支持 ARM 虚拟化，不代表 Android 内核、EL2 所有权、设备节点、系统服务和应用权限允许第三方 App 创建 VM。

## 2. 分层能力探测

Native Core exposes a bounded `/dev/kvm` probe through the C ABI and JNI. It reports the compile-time host architecture, whether the process can open `/dev/kvm`, the KVM API version, and the captured `errno` when access fails. The probe never creates a VM and never requests elevated privileges. `AVAILABLE` means only that the device node was opened and `KVM_GET_API_VERSION` returned the expected interface version; it does not establish AVF authorization, `KVM_CREATE_VM` permission, EL2 ownership, or Guest boot capability. `ACCESS_DENIED`, `UNAVAILABLE`, `INCOMPATIBLE`, and `ERROR` remain distinct outcomes. The Android layer must combine this evidence with API/service/permission probes before selecting a backend.

每项能力分别记录 silicon、firmware、kernel、device node、service、app permission、backend API 和 end-to-end 状态。CPU feature 检测结果不能直接当作 KVM/AVF 可用结论。

## 3. Backend 选择

Kotlin runtime API 中的 `HostCapabilityGate` 是保守决策器，输入必须是实际采集的证据，而不是设备型号推断。AVF 必须同时具备 API、服务、应用授权及目标 VM 类型支持；直接 KVM 必须同时具备 AArch64、KVM API v12、VM 创建授权、EL2、Stage-2 和 ARM GIC 证据。未知、拒绝、错误或不兼容均视为 blocker。决策器只在完整满足条件的硬件后端中按偏好选择；没有满足条件的后端时返回 unsupported，不存在 CPU 模拟或容器回退。当前 JNI 探测只提供 KVM device/API 证据，其余字段必须由后续 Android/Hypervisor adapter 以真实系统证据填充，不能伪造为 AVAILABLE。

### 3.1 能力证据与决策边界

- `CapabilityFact.UNKNOWN` 不等于不可用，也绝不等于可用；必须继续探测或报告未知。
- `/dev/kvm` 可打开且 API 版本正确，只能填充 KVM device/API 证据。
- 应用是否被允许创建 VM、EL2 是否由可用 Hypervisor 管理、Stage-2 与 GIC 能力，必须由受支持的系统接口或授权后端证明。
- AVF service/API/授权状态不能通过检查包名或 Android 版本推断。

AVF：检测设备实际提供的 API、VirtualizationService、权限和支持的 VM 类型。AVF API/权限受 Android 版本、系统镜像、设备实现、应用预装/特权授权和具体 API 约束，不能简单规定 Android 13+ 所有手机可用。

直接 KVM/VMM：需要 Host Kernel 暴露 KVM 接口、权限、EL2/虚拟化控制权和兼容的 VMM/设备后端。/dev/kvm 是否存在只是检查项，不代表 App 可访问或创建所需 VM。crosvm 仅是 VMM 候选，不是 Hypervisor；可在 Android ARM64 KVM 上运行也必须通过系统权限和设备闭环验证。

厂商 Hypervisor：Gunyah、GenieZone 等接口受 SoC、厂商、内核和系统集成约束，不能当作 Android 通用标准，需独立适配、授权和真机验证。

软件 CPU 仿真（包括 QEMU TCG）不属于 ZeroDroid 运行后端，也不得作为完整 Guest 的自动 fallback。没有可用硬件虚拟化时，应明确报告设备不支持完整 VM。

### 3.2 能力探测执行框架

`HostCapabilityDiscovery` 通过独立的 architecture、AVF、KVM probe 接口收集证据，普通探测异常会转成对应的 `ERROR` 事实并附带来源与异常类型；协程取消会继续向上传播，不会被吞掉。探测器本身不推断 API level、设备型号或包是否安装，也不提供默认成功的 Android 实现。平台适配器必须在其实现前完成权限/API 审计，并只返回由真实系统接口验证的事实。报告同时返回原始证据、后端决策和探测失败记录，便于 UI/日志解释阻断原因。

### 3.3 AVF Android API 的真实可用边界（AOSP）

- AVF Java API 位于 `android.system.virtualmachine`，从 Android 14 / API 34 引入；这些接口是可选的 `@SystemApi`，不属于普通应用可依赖的稳定公开 SDK。
- 创建/运行 VM 需要 `android.permission.MANAGE_VIRTUAL_MACHINE`。该权限为受限权限：Android 14 仅平台特权应用可用；Android 15 起可授予预装应用；AOSP 开发测试环境可通过 ADB 授予。仅在 Manifest 声明权限不会获得授权。
- `PackageManager.hasSystemFeature("android.software.virtualization_framework")` 只证明设备声明了 AVF Framework Feature。它不能证明 VirtualizationService 可访问、当前 APK 已获授权、protected/non-protected VM 能力，或目标 Guest 可以启动。
- 在受支持的系统集成中，应通过 `VirtualMachineManager` 的实际实例及 `getCapabilities()` 区分 protected VM（capability bit 1）和 non-protected VM（capability bit 2）。无法合法调用该 System API 时，service、VM type 和 create permission 必须保持 UNKNOWN/ACCESS_DENIED，不能用设备型号、API level 或 Feature 位代填。
- 标准 AVF 应用 API 的主要工作流是运行 Microdroid，并执行 APK 中的 Native payload。它不是普通第三方 APK 可任意指定自定义 Kernel、Bootloader、完整 ROM 和设备树的通用接口。AOSP 的自定义 VM 配置还受 `USE_CUSTOM_VIRTUAL_MACHINE` 等受限权限约束。
- Microdroid 是精简 Android：提供 Bionic/部分 NDK、Verified Boot、SELinux、Binder RPC over vsock、AuthFS、APEX 和 APK payload；不提供 Android `android.*` Java API、SystemServer、Zygote、Graphics/UI 或 HAL。不能把“Microdroid 启动成功”报告为完整 Android ROM 已启动。
- 因此 ZeroDroid 必须分开建模：①普通 APK 可公开探测的 AVF feature/权限证据；②平台/预装授权后可调用的 AVF Java API；③平台级自定义 Guest 启动能力；④完整 Android Guest 的 Kernel、DTB、Vendor、图形与设备桥接能力。只有对应层全部通过验证，才允许报告相应能力。

### 3.4 Android 应用侧 AVF 探测实现

`app/.../AndroidAvfCapabilityProbe.kt` 只使用公开 SDK 可验证的系统 Feature 和当前应用权限。由于 AVF Manager 属于 System API，普通应用适配器不通过隐藏 API 反射访问它；Feature 存在时，若无法取得受支持的特权适配器证据，仍将 service 和 VM type 标记为 UNKNOWN。该实现是安全门控的一部分，不是 AVF VM 启动后端。

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
