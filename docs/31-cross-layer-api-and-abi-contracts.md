# 31. 跨层 API、JNI、C ABI 与 Backend 接口契约

## 1. 目的

本文定义 Kotlin、JNI、C ABI、C++ Runtime、Rust 辅助模块与 Backend 的调用边界。规范不代表未实现的功能已经落地。

## 2. 分层依赖方向

UI/Compose → Kotlin UseCase/Repository → VM Controller/Foreground Service → JNI Facade → ZeroDroid C ABI → C++ Runtime → Backend Adapter → AVF 或获得系统授权的 ARM64 KVM/VMM/厂商 Hypervisor。禁止软件 CPU 模拟路径。

镜像解析、Guest Profile、资源规划和设备清单属于领域服务。Guest VirtIO 协议独立于 JNI API。

## 3. C ABI

- 导出符号统一使用 zdroid_ 前缀。
- 公共结构体包含 abi_version 和 struct_size。
- 不暴露 C++ class、STL、异常、Rust trait 或编译器特定布局。
- 所有长度、嵌套深度和分配规模有上限。
- 所有函数返回稳定 status code，详细诊断通过 error_info 获取。
- 每个输出对象明确所有权。
- 公共头文件必须同时通过 C 与 C++ 编译器编译；ABI 布局通过静态断言验证。
- 本次 Native Host Capability 结构新增 vm_create_state/vm_create_errno，C ABI 主版本提升至 2；JNI 整数载荷同步扩展为 7 个字段。调用方必须通过 abiVersion() 检查版本，并按结构版本解析，不得假定旧版结构大小。

## 4. JNI

JNI 仅负责类型转换、线程衔接和错误映射。Kotlin 不接触 C++ 对象、STL 或裸指针。每个 JNI 入口检查输入并捕获 C++ 异常；不得让异常跨越 JNI frame。

## 5. Rust/C++ 互操作

Rust 仅通过 C ABI 或明确限定的静态库边界与 C++ 通信。当前 Rust 解析器保持纯 Rust、无 unsafe、无 FFI；未来接入时 repr(C) 结构固定布局、panic 必须在 Rust 边界捕获、跨边界资源由单侧释放，不跨 FFI 传递 String、Vec、Box、trait object 或 STL。

## 6. 不可信镜像输入

镜像解析器先检查输入字节预算，再检查结构范围、整数溢出、元素数量和展开后输出大小。稀疏镜像索引只保存输入 payload 范围，不提前展开或复制大块数据。结构校验、CRC、AVB 签名验证和实际分区写入是独立阶段，不得将结构合法误报为镜像可信。

## 7. 线程与资源所有权

VM 控制操作串行化；不同 VM 可并行但受全局预算约束。fd、mmap、共享内存、虚拟磁盘、socket、线程和 JNI reference 必须有明确所有者。销毁顺序先停止 Guest，再关闭设备通道，最后释放后端资源。

## 8. 版本兼容与测试门禁

分别版本化 Kotlin API、C ABI、DTO schema、Backend capability schema、VirtIO device ABI 和 bridge protocol。C ABI 使用 struct_size 支持尾部扩展。测试覆盖 C/C++ 头文件互操作、ABI 布局、句柄 stale/重复释放、JNI 输入错误、Rust panic 边界、并发操作、解析器边界与资源预算。进入 DEVICE_VALIDATED 前必须记录具体设备、系统版本、权限和 Backend。
