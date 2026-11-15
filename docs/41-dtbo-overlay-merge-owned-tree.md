# DTBO Overlay Merge：有界拥有树与原始输入事务

本模块提供低层可控合并接口，并提供 `apply_dtbo_sequence` 作为面向原始 DTB/DTBO 输入的有序、原子执行入口。

## 核心数据与接口

- `FdtOwnedProperty` / `FdtOwnedNode`：合并结果拥有属性字节，不引用调用者的 DTB/DTBO 缓冲区。
- `FdtMergedTree`：承载 reservation map、节点拓扑、根 compatible/model 和 boot CPU ID。
- `merge_overlay_fragments`：按已验证的 resolution 顺序合并 fragment。
- `merge_overlay_sequence`：低层预解析事务接口，调用者为每个 Overlay 提供针对累计树计算的 resolution。
- `apply_dtbo_sequence`：原始 DTBO 输入的完整自动流水线；调用者不需要手动预先生成 fixup plan 或 target resolution。

## 完整执行流水线

每一个 Overlay 都严格针对当前累计 DTB 处理：

1. 重新解析当前累计 DTB 和原始 DTBO，建立新的 FDT 索引。
2. 扫描基础树 phandle，验证格式、重复项和保留值，计算当前最大 phandle。
3. 生成 external fixup、local fixup，以及 Overlay 节点自身 `phandle` / `linux,phandle` 声明的 relocation 计划。
4. 将所有写入汇总为一个按绝对偏移排序的 write set，拒绝重复或重叠的 32-bit 写入。
5. 在分配输出副本前验证所有写入边界和预算，然后只修改私有 DTBO 副本；输入字节保持不变。
6. 重新解析修复后的 DTBO，提取 fragment，解析 `target` / `target-path`。
7. 将 fragment 子树和 `__symbols__` 导出路径合并到拥有树。
8. 合并 reservation map，重新验证 phandle 全局唯一性。
9. 序列化为 FDT v17，并重新解析验证结果。
10. 下一项 Overlay 使用上一步输出作为基础，重新计算 phandle relocation delta；全部完成后再执行最终解析并返回完整 DTB。

任一阶段失败都会返回错误，不会返回部分 DTB，也不会修改调用者输入。当前接口为：

```rust
pub fn apply_dtbo_sequence(
    base_data: &[u8],
    overlays: &[&[u8]],
    budget: ParseBudget,
) -> Result<Vec<u8>, FdtError>
```

## 合并语义与冲突策略

- 同名属性替换原值；新属性追加。
- 同名子节点递归合并；不存在的子节点及其后代整体复制。
- fragment 按 DTBO 中解析到的顺序处理。
- reservation map 使用有界并集：按地址排序，完全相同的条目去重；零长度、地址加长度溢出、不同条目重叠均拒绝；相邻但不重叠的区间保持独立。
- Overlay 的每个 phandle 声明与 local fixup 引用使用相同 delta relocation，并在同一个 write set 中原子写入。
- 合并完成后再次检查 phandle / linux,phandle 格式、别名一致性、保留值和全局唯一性。
- `__symbols__` 中 `/fragment@N/__overlay__` 路径会映射到最终目标路径；同名符号冲突拒绝。

## FDT 版本与序列化

解析器分别处理 FDT v16 和 v17 header：v16 不读取 v17 的 `size_dt_struct` 字段，而是按结构 token 扫描到 `FDT_END` 推导结构块长度；v17 使用 header 中的结构块长度。两者均执行 block 范围、对齐、reservation map、token、节点、属性和预算校验。

`serialize_fdt_v17` 生成唯一 strings table、structure block、8-byte 对齐 reservation map 和 v17 header，并在返回前 round-trip 检查节点路径、父索引、属性字节、reservation map 和 boot CPU ID。

## 资源与安全边界

- 输入总量、节点/属性/reservation 条目、属性复制字节数、Overlay 数量和嵌套深度均受 `ParseBudget` 限制。
- 结构长度、偏移、phandle relocation、reservation end 使用 checked arithmetic。
- 不接受伪造、遗漏、重复的 resolution；低层 merge 入口重新解析输入并校验传入索引与原始字节一致。
- fixup 写回仅发生在私有副本；调用方输入不变。
- 本流水线不验证 AVB/VBMeta 签名、不认证镜像来源，也不证明 Guest 可启动。

## 已覆盖的内部回归测试

- FDT v16 header 独立解析及 structure 长度推导。
- FDT v17 serializer round-trip。
- phandle 声明和 local 引用同步 relocation。
- external symbol fixup、写入冲突和边界拒绝。
- reservation map 去重、排序和重叠拒绝。
- 原始 DTBO 自动执行 fixup→resolve→merge→serialize。
- 多 Overlay 累计树顺序应用。
- 伪造 resolution、空 resolution、过期索引拒绝。
- Overlay symbol export 路径重写。

## 尚待外部验证的工作

- 导入真实 dtc/libfdt 生成的 v16/v17 DTB 和 DTBO fixtures。
- 与目标版本 Linux libfdt 的差分测试，覆盖多 fragment、external/local fixup、phandle relocation、symbols 和 reservation map。
- 将合并结果接入 Guest Platform Profile，验证 CPU、memory、GIC、timer、PSCI、UART、VirtIO-MMIO 的地址区间、IRQ、phandle 引用和设备依赖。
- 将验证结果接入 Boot Material Set 和启动链策略。

这些是外部互操作和 Guest 平台语义验证，不应与本模块内部事务实现混为一谈。
