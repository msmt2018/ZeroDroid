# DTBO Overlay Merge：有界拥有树

本阶段在已经完成 FDT 结构解析、fragment target 路由和 external/local fixup 写回之后执行节点合并。

## 接口

- `FdtOwnedProperty` / `FdtOwnedNode`：拥有属性字节，避免合并结果继续引用原始 DTB/DTBO 缓冲区。
- `FdtMergedTree`：承载基础树 reservations、合并后的节点及根节点兼容信息。
- `merge_overlay_fragments(...)`：按传入 resolutions 的顺序，将每个 fragment 的 `__overlay__` 子树合并到基础树目标节点。

## 合并语义

- 同名属性替换原值；新属性追加。
- 同名子节点递归合并；不存在的子节点及其后代整体复制。
- 不同 fragment 按调用者提供的顺序应用。
- 保留基础树 reservation map；overlay reservation 不自动并入。
- 合并结束后重新检查 phandle 的格式、保留值和全局唯一性。
- 根 compatible/model 从合并后的根属性重新解析。

## 资源与失败语义

- 输入长度、节点/属性总数、属性字节复制量及 overlay 深度均受 ParseBudget 限制。
- 在内存中的局部结果构建完成前不会返回可用的 FdtMergedTree；错误不会修改调用方输入。
- 目前没有增量/原地写入，也没有对外暴露部分合并结果。

## 尚未完成

这不是完整 DTB 输出。下一阶段仍需实现 canonical FDT v17 serializer：重新生成 strings/structure/reservation blocks，计算所有偏移和长度，执行预算检查，再通过 `parse_fdt` 对输出做 round-trip 验证。还需合并并重写 `__symbols__` 导出路径、定义多 DTBO 事务/冲突策略，并增加真实 dtc 生成的 DTB/DTBO fixtures。当前函数也要求调用者先完成 fixup 并提供可信的 target resolution；它不执行 fixup、不验证签名，也不证明 Guest 可启动。

## FDT v17 序列化与 round-trip

`serialize_fdt_v17(tree, budget)` 将拥有树编码为 FDT v17：

- 生成唯一 property-name strings table 和对应 offset。
- 按节点先属性、后子节点的顺序输出 BEGIN_NODE/PROP/END_NODE token。
- 输出 8-byte 对齐的 reservation map（含终止记录）。
- 重建 header 中的 structure/string/reservation offsets、长度、版本和 boot CPU ID。
- 在返回前重新调用 `parse_fdt`，核对节点路径/父索引/属性字节/reservations/boot CPU ID。

序列化输入拓扑、属性名、重复属性、保留区域及输出长度均受检查和 ParseBudget 限制。round-trip 失败时不返回 DTB。

这仍是结构序列化与自校验，不是 AVB 签名验证、DTB 来源认证或真实设备启动验证。Overlay `__symbols__` 导出合并与路径重写仍未完成；真实 dtc 生成的跨版本 fixture 和多 DTBO 事务测试仍是后续工作。
