# 29. ROM 导入、OTA 解析与分区转换流水线

## 1. 目标

从用户提供的 ROM 包安全提取系统材料，生成可审计的 Guest Image Set。导入器负责解析、转换和报告，不得默认破坏原始镜像或自动绕过安全校验。

## 2. 输入格式

按模块化 Parser 支持 raw partition images、Android Sparse、super.img/liblp、boot/vendor_boot/init_boot/dtbo/vbmeta、OTA payload.bin（全量与增量）、*.new.dat、*.new.dat.br、transfer list、ZIP/TAR 容器，以及有文档或验证依据的厂商 OTA 容器。

格式支持需登记 parser 版本和测试样本。未知格式进入隔离区，不执行包内脚本。Rust image core 已提供 boot.img v3/v4 与 vendor_boot v3/v4 的结构索引，并新增独立 init_boot v4 解析入口：init_boot 必须使用 v4 header、Kernel size 必须为 0、ramdisk 必须非空；其 payload 仅以有界 Range 索引，不复制或展开。该解析只证明镜像结构可读，不代表 boot signature、AVB 或 Guest 启动链验证通过。

## 3. 安全导入流水线

Input → Quarantine → Archive Inspection → Path Validation → Size/Ratio Limits → Hash → Format Detection → Metadata Parse → Partition Extraction → Image Validation → AVB Analysis → Dependency Graph → Profile Match → Transformation Plan → Output Verification。

防止路径穿越、符号链接逃逸、压缩炸弹、超大分配、整数溢出、恶意 protobuf、畸形 sparse block 和解析器崩溃。限制输入长度、解压比例、分区尺寸、嵌套深度和总磁盘占用。

## 4. OTA 语义

payload.bin 可能是增量 OTA，不能保证仅凭包内数据重建完整分区。增量 payload 需要匹配源分区与版本，并验证 source/target fingerprint、操作列表、源哈希和目标哈希。缺少基线时明确报告无法应用。

*.new.dat.br 需结合 transfer list 和对应分区版本解析；仅 Brotli 解压不代表得到完整可启动分区。

## 5. Sparse、LP 与分区映射

Sparse 转换保留逻辑块数量、块大小和填充语义；liblp parser 已实现 geometry/header/table SHA-256 校验、metadata slot 主备副本读取、group、partition attributes、extent、block device 和映射边界/容量约束索引；后续仍需补充 A/B slot 语义规范化、retrofit slot suffix 调整、snapshot/COW 语义与实际 super 镜像回归样本；重组前检查分区总量、group 上限、super 容量、slot 与 metadata 一致性。

不将 super.img 当作单一文件系统，不假设所有厂商分区都在 super 中；未知厂商分区名称应保留元数据，不得擅自删除。


### 5.1 统一 ROM 分区材料索引（新增）

Rust image core 新增 `partition_catalog`，将已解析的 GPT 物理分区、liblp 逻辑分区及单份 VBMeta 的 AVB 分区引用合并为只读材料索引。GPT 名称按 UTF-16LE 严格解码；LP 逻辑分区记录 group、逻辑扇区数和 backing block-device 名称；AVB Hash/Hashtree/Chain/Property/Kernel Cmdline 描述符继续使用对应语义解析器，未知 tag 保留为 Unknown，不伪造语义。

A/B 规范化必须由调用方显式选择 A、B 或 None。None 不推断后缀；选择 A/B 时无后缀名称映射到所选 slot，另一 slot 的显式后缀保持原样，不得把 `system_b` 错误映射成 `system_a`。索引报告 GPT 重复物理分区名、规范化重名、AVB 引用缺失和 Chain Partition 边；Chain 边的目标 VBMeta 尚未加载时明确标记 `target_vbmeta_loaded=false`，因此当前是依赖关系索引，不是完整递归信任链。

该目录构建只验证结构和引用关系，不执行分区数据提取、Hash/Hashtree 计算、公钥信任、签名或 rollback index 验证。其 `verification` 状态继承 VBMeta 的 `NotPerformed`，不得作为 Guest 启动放行条件。

## 6. 转换策略

先生成 Transformation Plan，记录输入分区/哈希、操作类型、依赖与兼容依据、输出分区/尺寸/哈希、对 AVB/VINTF/fstab/SELinux/HAL/OTA 的影响、可逆性和恢复方式。

优先保持原始分区不变。替换 Vendor、Kernel、DTB 或 vbmeta 属于高风险变换，只有匹配 Guest Platform Profile 且有验证证据时允许。禁止默认用同一份通用虚拟设备 Vendor 替换所有 ROM。

## 7. AVB 与 dm-verity

AVB image core 已新增 VBMeta v1.0–1.2 Header 与 Authentication/Auxiliary Block 的有界结构解析，索引 Hash、Signature、Public Key、Public Key Metadata 与 Descriptor 记录，并校验块边界、区域重叠、描述符长度、保留字段和解析预算。当前已完成 VBMeta v1.0–1.2 Header、Authentication/Auxiliary Block 和 descriptor TLV 外层索引，并实现 Hash（tag 2）、Hashtree（tag 1）、v1.0–1.2 旧布局 Chain Partition（tag 4）、Property（tag 0）及 Kernel Cmdline（tag 3）描述符字段语义解析。Hash/Hashtree 校验固定字段、保留字节、flags、UTF-8 分区名、算法标识、变量字段长度与 8-byte padding；salt/digest/root digest、公钥与 kernel cmdline 均以原始输入 Range 表示。descriptor 记录携带 VBMeta minor version，用于拒绝 v1.0 不支持的 flags 和 persistent digest 语义；Hashtree flags/persistent digest 按 v1.1+ 处理，Chain v1.3 flags 布局暂不支持。Property 校验 UTF-8 key/value、NUL 分隔和尾部 padding；Kernel Cmdline 校验 UTF-8、条件 flags 与 padding。仍未实现 digest/hashtree 计算、公钥信任链、签名验证、rollback index 与设备持久化比较。解析结果显式标记 `NotPerformed`，不得视为 AVB 验证通过。

不得把自动剥离 AVB、关闭 dm-verity 或清除验证标志设为通用导入步骤。开发测试若需修改镜像，必须用户明确选择、保留原始输入、标记测试态、说明安全后果并验证修改后的启动链。生产安全模型使用受控信任根和一致签名策略。

## 8. 输出与手机端资源

每次导入生成只读原包归档、分区哈希/大小/格式/状态、Partition Graph、Boot Material Set、AVB Report、Profile 匹配、Transformation Plan、执行日志、阻断项和最终 Guest Image Set。

大镜像处理使用流式 I/O、分块、临时文件配额和可恢复任务。内存预算按 Host 可用内存与压力动态计算并设硬上限，不能把数十 GB 镜像整体读入 RAM。

## 9. 验收

每个 Parser 有合法/畸形/边界样本、模糊测试和哈希回归；每次转换可复现并能解释输入输出差异。
