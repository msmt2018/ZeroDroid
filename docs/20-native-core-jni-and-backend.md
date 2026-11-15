# 20. Native Core、JNI 与虚拟化 Backend 架构

## 1. 设计目标

Native Core 提供稳定的 VM 管理、镜像处理、启动编排和设备抽象。ARM64 Guest 指令必须由硬件虚拟化扩展执行；Native Core/VMM 负责 KVM/AVF 接入、vCPU/内存/中断/设备运行管理，不得以 QEMU TCG 等软件 CPU 模拟替代。Hypervisor 能力来自 Host Kernel/固件/AVF，不是普通 App 可自行创建的功能。

## 2. Native 模块边界

- runtime：Runtime 初始化、全局依赖和关闭。
- vm：VM 对象、状态机、操作协调和句柄。
- backend/api：抽象接口和能力模型。
- backend/avf：AVF 适配。
- backend/kvm：经系统授权的 ARM64 KVM Backend 与 ZeroDroid VMM 设备运行时。
- backend/crosvm：可选 VMM 适配，不是 Hypervisor。
- boot：Guest Boot Profile、启动参数和 Boot Orchestrator。
- image：镜像探测、解析、验证和分区。
- hardware：虚拟设备 manifest、拓扑和 ABI。
- bridge：Host/Guest 数据通道。
- resource：预算、监控和压力策略。
- security：策略、密钥引用和安全审计。
- diagnostics：事件、日志、dump 和错误分类。

## 3. C++ 与 Rust 选型

C++20 用于 Android NDK/JNI、宿主系统调用、Runtime 和后续 VMM/Hypervisor 适配。Rust 用于不可信镜像输入的有界解析与索引构建，优先保持纯数据输入/输出和 forbid(unsafe_code)。只有明确的内存安全边界才跨 FFI。

## 4. 当前底层源码实现状态

- Native C ABI v1：Runtime opaque handle、状态码、错误结构、Host KVM 设备/API 探测。
- JNI：仅提供 ABI 版本和 Host 能力结果转换；尚未暴露 VM 创建/启动接口。
- Rust image core：checked range/alignment、输入/item/展开大小预算、Android sparse v1 结构索引与 CRC32 校验、Android boot image v3/v4 header/payload-range 索引，以及 Vendor Boot v3/v4 section/ramdisk-table/bootconfig 索引。Boot signature 当前只索引范围，不代表完成密码学认证；Vendor Boot parser 不解释 DTB、board selection 或 bootconfig 语义。
- C ABI header smoke test：使用 C 编译器编译公共头文件，并链接 C++ 实现。
- 尚未实现：sparse 数据写出、Vendor Boot/Init Boot、LP metadata/GPT/VBMeta 完整解析、AVB 验证、KVM VM/vCPU/Stage-2/GIC/设备运行后端和 Guest 启动。

## 5. Backend 接口

Backend 必须表达能力而非假定所有后端支持全部功能。真实接口需要异步操作、取消令牌、事件订阅、稳定 VM handle、错误域和版本协商。不支持的功能不得伪装成功。

## 6. JNI ABI 规范

Kotlin 不直接接触 C++ 对象、STL 类型或裸指针。JNI 仅负责类型转换、线程衔接和错误映射。所有 JNI 入口必须捕获 C++ 异常，不让异常越过 JNI 边界。

## 7. 线程和资源所有权

- 不在 Android 主线程执行镜像解析、磁盘 I/O、启动等待和日志读取。
- VM 每实例使用串行操作队列或状态机操作锁。
- fd、mmap、共享内存、虚拟磁盘、socket、线程和 JNI reference 必须有明确所有者。
- 销毁顺序先停止 Guest，再关闭设备通道，再释放后端对象和磁盘锁。

## 8. Backend 适配策略

AVF 与直接 KVM/VMM 的配置模型不同。上层使用统一 Guest Profile 和 Virtual Hardware Manifest；Backend Adapter 将其转换为 AVF 或 KVM/VMM 配置，并返回实际协商后的能力与资源。QEMU/TCG、Android Emulator、Goldfish、AEMU 和 Cuttlefish 模拟器启动链不在候选后端中。

## 9. Native 构建与依赖

Android App 使用 Gradle/AGP/Kotlin；JNI/C++ 使用 Android NDK + CMake；Rust 模块通过 Cargo/NDK target 集成。上游 crosvm/AOSP/Kernel 构建体系可能需要 Soong、Bazel、Ninja，不能假设直接作为 CMake 子目录。

## 10. Android ARM64 移植要求

Native Core 不得把 Linux PC Host 依赖直接带入 APK。所有 VMM 代码按 Portable Core、Android Host Adapter、Hypervisor Adapter 分层；PC 专属 x86/PCI/ACPI/APIC/BIOS 设备后端不能进入生产路径。

## 11. 测试

- Native 单元测试与 C ABI header smoke test。
- Rust parser 单元测试及后续 cargo-fuzz。
- Fuzz：Boot Image、Sparse、LP metadata、GPT、VBMeta 等不可信输入。
- JNI 测试：异常、重复释放、线程退出和 GC。
- Backend Contract Test 与目标真机验证。
