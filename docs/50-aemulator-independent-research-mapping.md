# 50. AEmulator 技术研究到 ZeroDroid 独立实现的映射

## 目标与边界

AEmulator 是 ZeroDroid 的重要技术研究对象。研究其 ARM64 原生执行、Guest 用户态环境、ROM 导入、Android 版本适配、Binder、图形、音频和网络桥接的设计经验，用于识别真实问题、接口边界和兼容性风险。

ZeroDroid 的目标不同：运行独立 ARM64 Guest Linux Kernel、Guest init、Binder 驱动、Android Framework 和虚拟设备。AEmulator 的用户态兼容运行环境不能替代完整 Guest VM，也不能作为 ZeroDroid 的生产运行后端。

**独立实现规则：** 不复制、移植、改名或直接使用 AEmulator 的源码、二进制、生成物、补丁或运行时依赖。只借鉴公开可观察的架构思路和问题解决方法；ZeroDroid 的实现、协议、代码和测试独立编写。AEmulator 上游项目许可证为 GPL-3.0，本项目不得把其代码混入 ZeroDroid。

## 已研究的技术方向与独立实现映射

| AEmulator 研究方向 | 可借鉴的工程经验 | ZeroDroid 独立实现 |
|---|---|---|
| ARM64 原生执行与 Guest ABI | Host/Guest ABI、动态链接、系统调用和架构边界 | guest-abi、Guest Platform Profile；Guest Kernel 处理系统调用，VMM 负责 vCPU 执行 |
| Guest linker / bionic 环境 | Guest 用户态必须使用与 ROM 匹配的 linker、libc 和动态库 | 由真实 Guest system/vendor 镜像提供其 linker 与 bionic，不把 Host Bionic 冒充 Guest |
| seccomp / SIGSYS 路由 | 系统调用边界、安全过滤和 Host/Guest 责任划分 | Host VMM 自身 syscall sandbox；Guest syscall 由独立 Guest Kernel 处理，不用 SIGSYS 兼容层取代 Guest Kernel |
| ROM Importer | ROM 容器识别、版本差异、组件提取和导入前检查 | Rust zerodroid-image-core 与独立 ROM pipeline：ZIP/TAR、payload、sparse、boot、vendor_boot、init_boot、super、LP、AVB、DTB/DTBO |
| GuestVm / InitPlan | 启动依赖、服务顺序、失败诊断和版本化启动策略 | Guest Boot Orchestrator；真正启动 Guest Kernel 和 ROM 内 init，不在 Host 上模拟 Android init |
| 用户态 Binder | Binder transaction、对象引用、FD、buffer、线程池和服务发现 | 优先使用 Guest Kernel Binder 驱动和 Guest servicemanager；仅对 Host/Guest 控制通道实现独立受限协议 |
| GL Bridge / Gralloc / HWC | 图形协议边界、Buffer 生命周期、格式/同步/显示兼容 | 独立虚拟 GPU 设备协议和 Host GPU Adapter；不得默认零拷贝或降低 Guest 分辨率 |
| Audio / Network / Storage bridge | Host 资源代理、格式转换、缓冲、权限和生命周期 | VirtIO 或版本化设备协议及最小权限 Host Adapter |
| Android 版本化 Guest Runtime | Android 版本、ROM 组件和 ABI 兼容矩阵 | Guest Platform Profile + Kernel/HAL/VINTF/Framework 版本组合 |

## 对当前底层核心开发的直接影响

1. ROM 导入不是简单解压：必须解析真实 Android 容器和分区语义，校验大小、哈希、AVB、LP metadata、boot header、DTB/DTBO 和增量 OTA 基础镜像。
2. Android 版本适配必须数据驱动：每个 Guest Profile 记录 Android release、API level、Kernel、VINTF、HAL、分区布局和启动材料，不使用单一通用启动参数。
3. Host 与 Guest 必须分层：Host 进程的 seccomp、Bionic、SELinux 和 Android 服务不等于 Guest 的对应组件。
4. Guest Binder 必须是真正 Guest Kernel Binder：不能因为用户态 Binder 可以跑通测试，就宣称完整 Android Framework 已经兼容。
5. 图形桥接属于虚拟设备和显示后端的一部分：Guest GPU/显示驱动、Buffer ABI、同步协议、Host EGL/Vulkan 和 Surface 生命周期必须闭环。
6. AEmulator 不具备独立 Guest Kernel 的完整 VM 语义；其没有覆盖的 Stage-2、GIC、虚拟 Timer、Guest IPA、VirtIO transport 等必须由 ZeroDroid 自己实现或接入经过审计的 VMM/Hypervisor。

## 研究与验证纪律

- 对 AEmulator 的结论必须来自可复核的公开源码结构、文档或实际测试；推断与事实分开记录。
- 不把 AEmulator 支持的 ROM 格式直接等同于 ZeroDroid 已支持。
- 不把 AEmulator 的 Android 版本范围直接等同于 ZeroDroid Guest Kernel/HAL 兼容范围。
- 不将其 GL Bridge 的性能或画质作为 ZeroDroid 已达成的指标。
- 每项借鉴必须对应 ZeroDroid 自己的模块、源码、测试和验收条件。
- AEmulator 只作为研究输入，不进入第三方依赖 manifest、APK、运行时或构建产物。

## 当前实现状态

本文件是研究到实现的映射规范，不代表所有映射模块已经完成。当前可验证的底层代码以 cpp/、native/、rust/ 源码及 CI 为准；Guest Kernel 启动、生产 AVF/KVM 后端、vCPU 执行、虚拟 GIC/Timer、VirtIO 设备闭环和 Android Framework 启动仍需独立实现和真机验证。