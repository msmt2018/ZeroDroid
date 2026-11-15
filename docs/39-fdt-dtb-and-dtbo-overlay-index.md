# 39. FDT/DTB 结构索引与 DTBO Overlay 路由元数据

## 1. 范围

`zerodroid-image-core::fdt` 提供有界、只读的 Flattened Device Tree 解析。输入 DTB/DTBO 的 property payload 不复制，节点和属性保留索引与原始输入范围。

解析器现在分别处理 FDT v16 与 v17 header：v17 使用 `size_dt_struct`，v16 不读取该字段，而是按 token 边界扫描到 `FDT_END` 推导 structure block 长度。两种版本共用后续结构校验，但不会混用 header 布局。

## 2. 校验

- FDT magic、total size、version/last compatible version。
- structure/string block 范围、4-byte structure alignment 和块间重叠。
- reservation map 的 8-byte 对齐、完整 16-byte 记录、终止记录、非零 size 与 address+size 溢出。
- BEGIN_NODE/END_NODE/PROP/NOP/END token 顺序、单根节点、平衡深度和终止 token。
- 节点名称 UTF-8、节点名称 padding、property value padding。
- property name offset 和字符串表 NUL 终止。
- 同一节点重复 property 名称拒绝。
- 输入大小、节点/属性/保留区条目总数和嵌套深度预算。

## 3. 平台识别

根节点的 `compatible` 字符串列表和 `model` 被提取为平台身份信息。兼容性判断使用调用方 allow-list 的精确字符串匹配，不做模糊前缀匹配。

## 4. DTBO 当前能力边界

`index_overlay` 提取根级 `fragment@*` 的 `target` 或 `target-path`、`__overlay__` 子树是否存在，以及 `__fixups__`/`__local_fixups__` 是否存在。

Overlay 执行流水线现已具备。底层仍保留只读 target 解析接口：对 base DTB 建立排序后的 phandle 索引，拒绝无效/重复 phandle，并将 fragment 的 target-path 或 target phandle 精确映射到 base 节点；fragment 缺少 `__overlay__` 子树时拒绝生成解析结果。该阶段不修改任一 DTB。

`apply_dtbo_sequence` 已将 phandle relocation、external/local fixup 写回、target resolution、owned-tree merge、symbol export 合并和 v17 输出串成原始 DTBO 有序流水线。流水线成功只证明结构与内部事务校验通过，不代表设备树与 Guest machine 兼容或 Guest 可启动。

## 5. 下一步

- 导入真实 dtc/libfdt 生成的 v16/v17 fixture，完成跨实现互操作验证。
- 导入真实 dtc/libfdt DTB/DTBO fixtures，并对解析、fixup 和应用结果执行跨实现验证。
- 建立 Guest Platform Profile 的 compatible、CPU、memory、interrupt-controller、timer、GIC、PSCI、UART、VirtIO 节点约束。
- 将解析结果接入 Boot Material Set，同时继续保持启动链验证状态独立。
