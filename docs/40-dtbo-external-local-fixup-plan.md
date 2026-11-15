# 40. DTBO External/Local Fixup 只读计划

## 1. 目标

本阶段把 DTBO 的 `__fixups__` 与 `__local_fixups__` 从“存在性标记”推进为可验证的只读重定位计划。输入 DTB/DTBO 均保持不变；返回值只描述后续独立输出缓冲区需要执行的写入。

## 2. External Fixups

- 从基础 DTB 的 `/__symbols__` 节点按符号名精确查找路径。
- 符号路径必须是以 `/` 开始的单个 NUL 结尾字符串，并且必须对应基础树中的节点。
- 目标节点必须具有有效的 `phandle` 或 `linux,phandle`。
- 按 DTBO fixup value 中的 `node-path:property:offset` 列表解析每个写入点。
- offset 按十六进制字节偏移解释，必须 4 字节对齐，并完整落在目标 property 的 32 位单元内。
- 不会直接写入目标 phandle，只在计划中记录符号、目标路径、目标 phandle 和 overlay property range。

## 3. Local Fixups

- 将 `/__local_fixups__` 下的镜像节点路径映射回 DTBO 原始节点路径。
- 每个 fixup property 必须是非空、4 字节数组，内容是待重定位 property 的字节偏移。
- 每个 offset 必须 4 字节对齐，且目标 32 位值完整位于对应 property 范围内。
- 基础树 phandle 必须有效且全局唯一。
- 以基础树最大 phandle 作为 relocation delta，使用 checked addition 计算新 phandle；溢出、保留值和无效原值均拒绝。
- 计划保留原 phandle 和新 phandle，不修改 DTBO。

## 4. 资源与安全边界

- External/Local fixup 条目共受 `ParseBudget.max_items` 限制。
- 所有输入 property range 都必须落在调用方提供的原始字节切片内。
- 所有 offset、range end 和 phandle 加法使用边界检查或 checked arithmetic。
- 目前没有执行 fixup 写回、overlay merge、symbol export 合并或最终 FDT 再解析。
- 计划生成成功不代表 DTBO 已应用，也不代表 Guest 设备树可启动。


## 6. 统一写入集与原子写回

实现增加 `FdtOverlayWriteSet`：

- External 与 Local fixup 被转换成绝对字节范围、目标 32 位值和写入类型。
- 写入集按绝对偏移排序；重复或重叠的 4 字节写入目标拒绝整个计划。
- 生成写入集时重新验证 property range、offset、输出长度及条目预算。
- `apply_overlay_fixup_write_set` 在分配输出前验证全部写入，再复制受预算限制的 DTBO 字节并执行写入。
- 原始 DTBO 不会被修改；任何验证错误都不会产生部分输出。
- 该写回只完成 DTBO fixup 字段填充，不等于把 overlay 子树合并进基础 DTB。

## 7. 尚未完成

- 将 fixup 写入后的 overlay 子树合并到基础树，并处理节点/property 覆盖语义。
- 完成 symbol export 合并与多 DTBO 顺序应用。
- 支持多个 DTBO 的有序应用、跨 overlay phandle 冲突与目标路径冲突检测。
- 对合并结果重新解析并执行 Guest Platform Profile 验证。
