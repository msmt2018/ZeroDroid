# 40. DTBO External/Local Fixup 与 phandle relocation

## 1. 目标与输入不变性

`plan_overlay_fixups` 对单个 DTBO 建立只读重定位计划；`apply_overlay_fixup_write_set` 只对独立的私有输出副本执行写回。原始 DTB/DTBO 字节不会被修改。

## 2. External Fixups

- 从基础 DTB 的 `/__symbols__` 按符号名精确查找路径。
- 符号路径必须是以 `/` 开始的单个 NUL 结尾字符串，并对应基础树中的节点。
- 目标节点必须具有有效 phandle。
- 解析 DTBO fixup value 中的 `node-path:property:offset` 列表。
- offset 按十六进制字节偏移解释，必须 4 字节对齐并完整落在目标 property 的 32-bit 单元内。
- 每个 external 写入记录符号、目标路径、目标 phandle、property range 和 offset。

## 3. Local Fixups 与声明重定位

- 将 `/__local_fixups__` 下的镜像节点路径映射回 DTBO 原始节点路径。
- 每个 fixup property 必须是非空、4 字节数组；每个 offset 必须对齐且位于目标 property 内。
- 基础树 phandle 必须格式有效、全局唯一；当前累计树最大 phandle 作为 relocation delta。
- 除 local reference 外，Overlay 节点自身的 `phandle` / `linux,phandle` 声明也必须加上相同 delta。
- 同一节点的 phandle 与 linux,phandle 别名必须一致；Overlay 节点间重复 phandle、保留值和 checked-add 溢出均拒绝。
- 声明和引用 relocation 进入同一个写入集，保证最终引用值与目标节点声明一致。

## 4. 统一写入集与原子写回

- External、Local reference 和 phandle declaration 都转换为绝对 4-byte 写入范围。
- 写入集排序；重复/重叠目标拒绝整个计划。
- `FdtOverlayFixupPlan` 和 `FdtOverlayWriteSet` 使用封装字段，仅提供只读访问器；调用者不能自行伪造写入集。
- 在分配输出前验证范围、输出长度和条目预算，再复制受预算限制的 DTBO 并写入。
- 失败不会产生部分输出；输入字节保持不变。

## 5. 原始 DTBO 自动流水线

`apply_dtbo_sequence(base_data, overlays, budget)` 已将各阶段串联：

1. 解析当前累计基础 DTB 与原始 DTBO。
2. 针对当前基础树计算 phandle relocation。
3. 构建并原子应用 external/local/declaration 写入集。
4. 重新解析修复后的 DTBO，索引 fragments。
5. 解析 target phandle/path。
6. 合并 fragment 节点、属性、reservation map 和 `__symbols__`。
7. 序列化 FDT v17 并重新解析。
8. 下一项 Overlay 使用累计输出重新开始上述流程。

任一阶段失败均不返回部分 DTB。流水线成功不代表 AVB 已验证，也不证明 Guest 可启动。

## 6. 自动化验证与剩余工作

CI 使用仓库中的 DTS/DT overlay 源文件运行 `dtc -@` 生成真实 DTB/DTBO，并验证 external fixup、local fixup、phandle 声明/引用同步 relocation 和 symbol export。

剩余工作集中在更广泛的 Linux libfdt 差分互操作、复杂多 fragment/reservation fixtures，以及 Guest Platform Profile 的设备资源语义验证；不是 fixup 写回或基础 overlay merge 尚未实现。
