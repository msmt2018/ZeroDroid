# 44. Android 应用层：Kotlin、Jetpack Compose 与原生 Material 3

## 1. 强制技术基线

ZeroDroid Android Host 应用层统一采用：

- **Kotlin**：所有 Android 应用层、界面层、应用状态、ViewModel、Repository、平台适配和应用服务代码均使用 Kotlin。
- **Jetpack Compose**：界面使用 Android 原生 Jetpack Compose 声明式 UI，不再以 XML View / Activity 手工拼装作为新界面的开发方式。
- **Material 3**：采用官方 `androidx.compose.material3`，遵循 Android Material 3 的主题、颜色方案、Typography、Shapes、组件状态、动态颜色和无障碍语义。
- **Compose Compiler Gradle Plugin**：与 Kotlin 编译器版本保持一致；通过 Version Catalog 和 Compose BOM 统一依赖版本。
- **Native 互操作边界**：C/C++、Rust 负责底层运行时和性能敏感模块；Android UI 与系统服务编排仍由 Kotlin 实现，通过稳定的 Kotlin API/JNI ABI 调用 Native，不允许把 Native 逻辑塞进 Composable。

除系统必须的 Android Framework/NDK 接口外，不引入第三方 UI 框架替代原生 Compose/Material 3。不得使用 WebView/HTML 页面冒充应用原生界面。

## 2. 分层架构

推荐依赖方向：

```text
Compose UI (Screen / Component / Theme)
                 |
                 v
       ViewModel + StateFlow
                 |
                 v
       UseCase / Application Service
                 |
                 v
       Kotlin Repository / Runtime API
                 |
          JNI / C ABI boundary
                 |
                 v
        C++ Runtime / Rust Core
                 |
                 v
       AVF / authorized KVM / VMM
```

约束：
- UI 层只负责展示状态、接收交互、发出意图，不直接管理 VM fd、vCPU、Guest memory、block backend 或线程。
- ViewModel 暴露不可变的 `StateFlow<UiState>`，通过事件/意图接收 UI 操作；Composable 使用 lifecycle-aware state collection。
- UseCase 负责 VM 创建、启动、停止、重启、镜像导入、配置校验、硬件能力探测和日志查询等业务流程。
- Repository 封装数据持久化和 Runtime API；Native 调用必须在合适的 dispatcher/执行器运行，不能阻塞 Main Thread。
- JNI API 必须返回有类型的状态/错误，不以字符串日志作为唯一控制协议。
- Activity 保持单 Activity 或少量明确的系统入口，导航和主要屏幕由 Compose 管理；需要外部 Intent/系统权限时通过 Activity Result API 适配。

## 3. Compose UI 结构

推荐包结构（按功能域组织，可随规模调整）：

```text
app/src/main/java/dev/zerodroid/app/
  MainActivity.kt
  ZeroDroidApplication.kt
  navigation/
    AppNavigation.kt
    AppDestination.kt
  ui/
    theme/
      Color.kt
      Type.kt
      Shape.kt
      Theme.kt
    component/
      CapabilityStatusCard.kt
      VmStatusIndicator.kt
      ResourceUsageCard.kt
      ErrorMessage.kt
    screen/
      home/
      vm/
      images/
      hardware/
      settings/
      diagnostics/
  feature/
    home/
    vm/
    image/
    hardware/
    settings/
  di/
```

不要求为每个 Composable 建立独立文件；应按可复用性和功能边界拆分，避免过度碎片化。

## 4. Material 3 设计规范

- 统一使用 `MaterialTheme.colorScheme`、`typography` 和 `shapes`，禁止在各个页面硬编码颜色、字体和圆角。
- 优先采用 Material 3 的 Scaffold、TopAppBar、NavigationBar/NavigationRail、NavigationDrawer、Card、ListItem、Dialog、Snackbar、ProgressIndicator、SegmentedButton 等组件。
- 根据窗口尺寸使用自适应布局：紧凑宽度使用底部导航或抽屉，中等/扩展宽度可使用 NavigationRail 或侧边栏。
- 支持系统深色模式、动态颜色（可用时）、字体缩放、TalkBack、键盘导航和触控目标尺寸。
- VM 启动、停止、刷写、删除镜像等破坏性操作必须有明确的确认和进度/失败状态。
- 实时资源指标使用有界刷新频率和生命周期感知订阅，不允许 Compose 高频无意义重组。
- 复杂列表必须提供稳定 key；大列表使用 LazyColumn/LazyVerticalGrid；避免在 Composable 中做磁盘扫描、镜像解析或 JNI 阻塞调用。

## 5. 状态、生命周期和并发

- UI State 是单向数据流的唯一展示依据；Composable 不自行维护跨页面业务状态。
- VM 状态、镜像导入状态、硬件能力证据、日志事件应具有明确的数据模型和版本化字段。
- 对启动/停止/刷写等命令实施幂等性、取消策略、互斥和状态机约束，防止重复点击产生并发冲突。
- Activity/Composable 离开屏幕后应按订阅策略停止高频采样；VM 后台生命周期由明确的 Android Service/系统授权后端管理，不能依赖 Composable 存活。
- Native 事件通过有界事件流进入 Kotlin 层；背压、丢弃策略和终止事件必须明确。
- 错误信息向用户显示可操作的摘要，同时保留诊断 ID 和完整结构化日志。

## 6. 测试要求

- ViewModel：状态转换、错误映射、并发命令、取消和恢复测试。
- Compose UI：关键屏幕语义测试、交互测试、不同字体缩放和窗口尺寸测试。
- Screenshot/golden tests 可用于稳定组件，但不能替代功能测试。
- App integration：Kotlin Runtime API 与 JNI 的 ABI/错误映射测试。
- Accessibility：TalkBack 语义、可点击区域、颜色对比度和键盘焦点测试。
- CI 必须在 Android 构建成功后验证 APK 文件存在且非空，并将 APK 作为可下载 artifact 上传。

## 7. 构建与 APK Artifact 规则

Android 工作流应：
1. 使用仓库声明的 JDK、Gradle、AGP、Android SDK、NDK 和 CMake 版本。
2. 运行 Kotlin/Android 单元测试及 Native/Rust 必要测试。
3. 执行 `:app:assembleDebug`。
4. 检查 `app/build/outputs/apk/debug/app-debug.apk` 存在且非空。
5. 输出文件类型、大小和 SHA-256，便于定位构建产物。
6. 使用 `actions/upload-artifact` 上传 `ZeroDroid-debug-apk`，并设置明确的保留期限。
7. 若 APK 缺失，工作流必须失败，不能出现“构建成功但没有 APK”的假阳性。

Debug APK Artifact 是 CI 临时产物，不等于签名发布版。Release APK/AAB 需要单独的签名密钥管理、版本号策略、可复现构建和发布工作流；不得把签名密钥提交到仓库。

## 8. 当前代码状态

当前基础分支已将 MainActivity 从传统 TextView 改为 Compose Activity，并引入 Compose BOM、Material 3、Compose UI、Preview、Activity Compose 和 Compose Compiler 插件。当前主页仅为应用壳和宿主能力探测入口，不能描述为完整 VM 管理界面。

当前 CI 工作流已增加 Debug APK 输出校验与 Artifact 上传步骤。最终是否成功仍以对应提交的 Android CI job 和 artifact 列表为准。

后续 UI 开发应按 VM 管理、镜像仓库、Guest Profile、硬件能力、运行监控、日志诊断和设置等功能逐步实现，不允许在 Compose 页面中伪造尚未实现的 VM 功能。
