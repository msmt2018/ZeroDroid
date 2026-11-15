# 39. FDT/DTB 结构索引与 DTBO Overlay 路由元数据

## 1. 范围

`zerodroid-image-core::fdt` 提供有界、只读的 Flattened Device Tree 解析。输入 DTB/DTBO 的 property payload 不复制，节点和属性保留索引与原始输入范围。

当前严格接受 FDT v17 header。旧版本 header 的结构大小字段语义不同，暂不猜测或套用 v17 解释。

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

这不是 Overlay 应用器。当前已增加只读 target 解析：对 base DTB 建立排序后的 phandle 索引，拒绝无效/重复 phandle，并将 fragment 的 target-path 或 target phandle 精确映射到 base 节点；fragment 缺少 `__overlay__` 子树时拒绝生成解析结果。该阶段不修改任一 DTB。\n\n尚未完成 phandle 重定位、符号解析、external/local fixup 写回、overlay merge、多 overlay 顺序冲突检查和最终设备树一致性验证。不能将“DTBO 结构可解析”或“fragment target 可解析”视为“DTBO 已成功应用”或“设备树与 Guest machine 兼容”。

## 5. 下一步

- 支持 FDT v16 header 的独立兼容解析路径（不能复用 v17 header layout）。
- 完成 DTBO external/local fixup 解析、符号解析与 overlay 应用计划。
- 建立 Guest Platform Profile 的 compatible、CPU、memory、interrupt-controller、timer、GIC、PSCI、UART、VirtIO 节点约束。
- 将解析结果接入 Boot Material Set，同时继续保持启动链验证状态独立。
