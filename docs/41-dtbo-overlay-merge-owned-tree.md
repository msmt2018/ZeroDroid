# DTBO Overlay Merge：有界拥有树

本阶段提供独立节点合并 API，并由 `apply_dtbo_sequence` 将 FDT 解析、external/local fixup、fragment target 路由、节点与 symbols 合并、reservation map 合并和输出序列化组合为原始 DTBO 有序事务。

## 接口

- `FdtOwnedProperty` / `FdtOwnedNode`：拥有属性字节，避免合并结果继续引用原始 DTB/DTBO 缓冲区。
- `FdtMergedTree`：承载基础树 reservations、合并后的节点及根节点兼容信息。
- `merge_overlay_fragments(...)`：按传入 resolutions 的顺序，将每个 fragment 的 `__overlay__` 子树合并到基础树目标节点。

## 合并语义

- 同名属性替换原值；新属性追加。
- 同名子节点递归合并；不存在的子节点及其后代整体复制。
- 不同 fragment 按调用者提供的顺序应用。
- 合并基础树与 Overlay reservation map：按地址排序，完全相同的条目去重；地址加长度溢出、零长度和相互重叠的区间均拒绝；相邻但不重叠的区间保持独立。总条目数受预算限制，避免静默丢弃 Guest 必须保留的内存区域。
- 合并结束后重新检查 phandle 的格式、保留值和全局唯一性。
- 根 compatible/model 从合并后的根属性重新解析。

## 资源与失败语义

- 输入长度、节点/属性总数、属性字节复制量及 overlay 深度均受 ParseBudget 限制。
- 在内存中的局部结果构建完成前不会返回可用的 FdtMergedTree；错误不会修改调用方输入。
- 目前没有增量/原地写入，也没有对外暴露部分合并结果。

## 尚未完成

该阶段已经具备 FDT v17 serializer：重新生成 strings/structure/reservation blocks，计算偏移和长度，并通过 `parse_fdt` round-trip 验证。`__symbols__` 导出路径重写也已接入单次 merge。另已增加 `FdtOverlayBatchEntry` / `merge_overlay_sequence(...)` 有序事务入口：每个 overlay 以当前累计 DTB 重新解析后的索引为基准执行，序列化并 round-trip 后才进入下一项；失败时不返回部分树。调用者必须为每一项提供针对当时累计树计算的 resolutions，并预先完成该项 fixup。真实 dtc/libfdt 生成的 DTB/DTBO fixtures、与 libfdt 的差分互操作测试，以及 Guest Platform Profile 的设备资源语义验证仍待实现。reservation map 已采用有界并集与重叠拒绝策略；merge 入口还会重新解析并比对调用者传入的 FDT index，拒绝过期或与输入字节不匹配的索引；同时逐项验证 resolution 与 DTBO fragment 的真实 `target-path`/`target`、基础树目标节点 phandle 及 fragment 完整性，拒绝伪造、遗漏或重复的 resolution。当前函数也要求调用者先完成 fixup 并提供可信的 target resolution；它不执行 fixup、不验证签名，也不证明 Guest 可启动。

## Overlay `__symbols__` 导出合并

合并阶段现在会读取 Overlay 根节点的 `/__symbols__`：

- 将 `/fragment@N/__overlay__` 下的导出路径映射到对应 resolution 的目标路径。
- 保留基础 DTB 已有符号；新增 Overlay 符号写入基础树的 `/__symbols__`，必要时创建该节点。
- 要求源路径严格位于某个已解析 fragment 的 `__overlay__` 子树中，且映射后的目标节点确实存在。
- 同名符号映射到同一路径时视为幂等；映射到不同路径时拒绝合并。
- 基础树已有符号路径也会验证为绝对路径并指向当前树中的节点。

该处理只负责合并后的导出符号路径，不代替 external/local fixup 写回。`apply_dtbo_sequence` 已把 fixup、target 解析、merge、序列化组合为原始 DTBO 的有序事务入口；低层独立 API 仍保留给需要显式控制阶段的调用者。

## FDT v17 序列化与 round-trip

`serialize_fdt_v17(tree, budget)` 将拥有树编码为 FDT v17：

- 生成唯一 property-name strings table 和对应 offset。
- 按节点先属性、后子节点的顺序输出 BEGIN_NODE/PROP/END_NODE token。
- 输出 8-byte 对齐的 reservation map（含终止记录）。
- 重建 header 中的 structure/string/reservation offsets、长度、版本和 boot CPU ID。
- 在返回前重新调用 `parse_fdt`，核对节点路径/父索引/属性字节/reservations/boot CPU ID。

序列化输入拓扑、属性名、重复属性、保留区域及输出长度均受检查和 ParseBudget 限制。round-trip 失败时不返回 DTB。

这仍是结构序列化与自校验，不是 AVB 签名验证、DTB 来源认证或真实设备启动验证。真实 dtc/libfdt 生成的跨版本 fixture 和跨 overlay reservation/conflict 测试仍是后续工作。

## 回归测试补充

增加了端到端的合成测试：先生成基础 DTB 与含 `target-path` fragment 的 DTBO，再解析索引、执行 fragment merge，确认同名属性替换和新子节点插入，最后序列化并重新解析输出 DTB。该测试验证的是本仓库 FDT v17 编解码与合并阶段之间的接口闭环；它不是对 dtc/libfdt 兼容性的替代，仍需导入真实工具链产物进行互操作测试。
