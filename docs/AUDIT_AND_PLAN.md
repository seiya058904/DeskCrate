# DeskCrate：源码审计与 Rust 重构方案

> 审计对象：`Tianyu199509/DeskBox` @ `cb4d252`（克隆在 `.ref/DeskBox`，已被 `.gitignore` 排除，和本仓库的 Git 历史互不相干）。
> 审计日期：2026-11-22。

## 0. 结论速览

| 项目 | DeskBox（原版） | DeskCrate（目标） |
| --- | --- | --- |
| 技术栈 | C# + WinUI 3 + .NET 10 Native AOT + Windows App SDK 2.7 + Rust Shell 层 + QuickJS | 纯 Rust + Win32 + DWM + Direct2D/DirectWrite（只用系统 DLL） |
| 代码规模 | 1193 个 `.cs` 文件，约 66.3 万行；60 个 XAML 文件 | 预计 1–1.5 万行 Rust |
| 分发体积 | 安装包 32 MB（内含私有的 Windows App Runtime） | 单个便携 `.exe`，< 5 MB（预估 1–2 MB） |
| 常驻内存 | 官方 Release 稳态约 115 MB 私有内存、171 MB 工作集（见 CHANGELOG） | 空闲 < 20 MB，折叠或隐藏时主动释放缓存 |
| 进程模型 | 主进程，加上设置/Studio 伴随进程、缩略图代理进程、MCP stdio 桥 | 单进程、单 UI 线程，外加 1 个按需工作线程 |

**选定方案**：Win32 顶层窗口。材质、圆角和阴影交给 DWM 系统背板（`DWMWA_SYSTEMBACKDROP_TYPE` 和 `DWMWA_WINDOW_CORNER_PREFERENCE`）；内容用 Direct2D **软件**渲染到共享 DIB，文字用 DirectWrite。这样磨砂和圆角由 DWM 在它自己的进程里合成，我们的进程只在内容变化时重绘一次，空闲时 CPU 占用为 0。

## 1. 许可合规

- 原仓库：自 1.6.0 起为 `GPL-3.0-only`，另有商业授权（双授权），见 `LICENSE` 和 `LICENSING.md`。
- DeskCrate 移植了原版的视觉参数、布局公式和交互策略，按 **GPL 衍生作品** 处理：
  1. 本项目采用 **GPL-3.0-only**，源码完整公开，保留版权声明和致谢。
  2. 依据 GPLv3 §7(e) 和 LICENSING.md §4.2：**不使用** `DeskBox` 名称、Logo 和图标资源（`deskbox.ico/svg`、`deskbox-mark-*`、`deskbox-tray-*`），图标重新绘制。
  3. README 写明“非官方衍生作品，与原作者无关联、未获其背书”。
  4. **不访问** `deskbox.fun` 的任何端点（更新、目录、反馈）。本项目本来就没有联网功能。
  5. 不复制 `catalog/` 下的模板和提示词，也不复制商业保留内容。

## 2. 视觉基准（从原版 XAML/C# 提取）

| 维度 | 原版取值（出处） | DeskCrate 实现 |
| --- | --- | --- |
| 材质 | 默认 Mica，可选 Mica Alt、Acrylic、Solid（`SettingsService.cs:570`） | DWM 系统背板：Mica、Tabbed、Transient；Win10 退回 accent blur 或纯色 |
| 背板透明度 | `WidgetOpacity = 0.80`，`MaterialIntensity = 0.65` | `appearance.opacity` 控制着色层的 alpha |
| 外圆角 | Round 档为 8 DIP；其余档位：Small 4、Large 16、Square 0（`WidgetCompactBoundsCalculator`） | Win11 用 `DWMWCP_ROUND`（8 px，带系统描边和阴影） |
| 内圆角（选中框/悬停） | Round 档为 4 DIP | 4 DIP 圆角矩形 |
| 标题栏 | 最小高度 40；`Padding 14,4,12,4`；图标与标题间距 8；分隔线 `Margin 12,0,12,4`，`Opacity 0.62` | 原样还原 |
| 文字色（深色） | Primary `#F5F5F5`；Secondary `#E8FFFFFF`；Tertiary `#CCFFFFFF`；Disabled `#8FFFFFFF`；分隔线 `#66FFFFFF` | 同左；浅色主题用对应的黑色 alpha |
| 图标格 | 图标 30 DIP；字号 11.5；文件名 2 行；密度 0.56；水平 0.40、垂直 0.60；名称宽度系数 0.36（`FileWidgetIconLayout`） | 移植布局公式（第 5.4 节） |
| 状态层（深色/浅色） | 悬停 `#14/#0F`；按下 `#20/#18`；选中 `#28/#20`；选中+悬停 `#38/#2D`；失焦选中 `#10/#0C`。深色用白色、浅色用黑色（`FileItemSurfaceStyleCache`） | 原样还原 |
| 选中描边 | 深色 `#8BFFFFFF`，浅色 `#72000000`，1 px | 原样还原 |
| 胶囊 | 高 42 DIP；宽度档 172/248/272；悬停展开延迟 360 ms，离开收起延迟 620 ms，动画 220 ms（Snappy 160、Slow 360） | 原样还原 |
| 吸附 | 窗口间距 5，胶囊栏边距 12 | 吸附阈值 8 DIP，间距 5 |
| 默认尺寸 | 280 × 400 DIP | 同左 |

## 3. 底层逻辑审计

### 3.1 桌面贴底与 `Win+D`（`Services/WidgetLayerService.cs`）

原版的实战做法：

1. 用 `EnumWindows` 找到 **父窗口是 `Progman` 或 `WorkerW`** 的 `SHELLDLL_DefView`，并排除 `CabinetWClass`、`ExploreWClass`、`#32770` 这类同样挂着 DefView 的资源管理器窗口和文件对话框。找不到时不强行创建 WorkerW，以免和登录时的图标布局恢复抢跑。
2. `SetWindowLongPtr(hwnd, GWLP_HWNDPARENT, defView)`：把格子设为桌面图标视图的 **被拥有窗口**。`Win+D` 会把桌面抬到前台，而被拥有窗口始终在 owner 之上，所以格子不会被隐藏。用的是 owner，不是 `SetParent`：子窗口拿不到 DWM 材质和圆角。
3. `HWND_BOTTOM` 加 `SWP_NOACTIVATE` 贴底。写 Z 序前先遍历下方窗口，判断是否已经“床铺到位”：如果下方只有自家窗口、壳窗口或被忽略的窗口，就跳过。原因是重复执行 `HWND_BOTTOM` 会重绘 DefView 的 owner 带，造成闪烁（#241/#249/#449）。
4. **激活抑制**：激活一个 DefView 拥有的窗口，会把整条 owner 带抬到其它应用之上（#454/#574）。因此，当格子被外部窗口遮挡时，`WM_MOUSEACTIVATE` 返回 `MA_NOACTIVATE`；格子完全露出时才允许激活，保证键盘流程（重命名、过滤输入）可用。
5. 收到 `TaskbarCreated`（Explorer 重启）后清除 DefView 缓存，再重新挂载。

DeskCrate 采纳第 1–5 点。另外补一条：Explorer 崩溃时 owner 被销毁，格子窗口可能随之失效。重建时如果 `IsWindow` 为假，就按模型重新创建窗口（模型与 HWND 解耦，重建成本很低）。

### 3.2 多屏与 DPI

- 原版在 manifest 中声明 `PerMonitorV2`，监听 `WM_DPICHANGED`、`WM_DISPLAYCHANGE`、`WM_SETTINGCHANGE(SPI_SETWORKAREA)`，用 280 ms 防抖后恢复布局，拖拽或缩放期间暂停恢复。位置按“显示器硬件身份 + 拓扑”记忆：拔掉屏幕时迁移，插回时复原。
- DeskCrate：位置存为 **显示器稳定标识（`QueryDisplayConfig` 拿到的 `monitorDevicePath`）+ 相对该屏工作区的 DIP 坐标**。屏幕不在时临时放到主屏并夹进可见区，但 **不改写** 记录，插回后自动复原。`WM_DPICHANGED` 时采用系统建议的矩形，并按新 DPI 重排、重绘。

### 3.3 Everything 联动（`Platform/EverythingNativeMethods.cs`）

- 原版用纯 Win32 IPC：`FindWindow("EVERYTHING_TASKBAR_NOTIFICATION")`，用 `WM_COPYDATA` 发送 `EVERYTHING_IPC_COPYDATA_QUERY2W (18)`，结果通过 `WM_COPYDATA` 回到隐藏的回复窗口；用 `WM_USER` 查询版本和 `IsDbLoaded`。不捆绑 Everything，也不依赖 SDK DLL。
- DeskCrate 用同样的协议，结果异步回到 UI 线程，没有轮询。Everything 没在运行时，只做本地过滤。

### 3.4 体积、内存和残留进程的根因

1. **框架基线重**：WinUI 3、XAML 和 Windows App Runtime 的进程基线就要几十 MB；每个格子是一个 XAML 窗口和一棵可视树，外加 Mica 控制器。原版 CHANGELOG 记录过“每开关一次设置窗口泄漏约 10 MB 原生 XAML 树”这类框架缺陷。
2. **功能面过宽**：天气、音乐（频谱、歌词）、剪贴板历史和 OCR、待办、Markdown、自定义组件（QuickJS）、Studio/AI、WebDAV、更新器、反馈等，带来 73 个文件中的定时器、轮询兜底和后台服务。
3. **多进程**：设置和 Studio 伴随进程、缩略图代理、MCP 桥。CHANGELOG 记录了“系统应用退出握手卡住导致退出后残留后台进程”，以及“升级时缩略图代理成为孤儿进程并锁文件”。
4. **运行时打包**：私有的 Windows App Runtime 和 .NET AOT 产物使安装包达到 32 MB。

DeskCrate 的对策：去掉 XAML 和 WinAppSDK；只保留文件相关功能；单进程，不派生子进程；退出路径固定为“销毁窗口 → 删除托盘图标 → `PostQuitMessage` → main 返回”，进程随 main 返回一同结束，不依赖框架的退出握手。

## 4. 渲染与窗口方案选型

| 方案 | 美学还原 | 体积/内存 | 维护成本 | 结论 |
| --- | --- | --- | --- | --- |
| WinUI 3 / WebView2 / Tauri / Electron | 高 | 极差（≥ 80 MB） | 中 | 硬约束禁止 |
| egui/iced（wgpu/GL） | 中（自绘风格） | 差：GPU 上下文 30–80 MB，没有 DWM 材质 | 中 | 否 |
| Slint | 中 | 中（2–4 MB） | 中，但 Shell 集成要自己补 | 否 |
| GDI+ 分层窗口（`UpdateLayeredWindow`） | 中：拿不到系统 Mica，模糊要自己算 | 每帧 CPU 合成 | 高 | 否 |
| **Win32 + DWM 背板 + Direct2D(软件)/DirectWrite** | **高**：材质、圆角、阴影都是系统原生，与原版同源 | **极低**：只用系统 DLL，共享一块 DIB | 低：一种窗口、一个渲染器 | **采用** |

### 关键技术点

- **窗口**：`WS_POPUP` + `WS_EX_TOOLWINDOW`（不进任务栏和 Alt-Tab）。调用 `DwmExtendFrameIntoClientArea(-1)`，然后把内容按预乘 alpha 绘制。
- **材质常亮**：桌面格子很少处于激活状态，而 Mica 在失活时会退化为纯色。原版通过 `SystemBackdropConfiguration.IsInputActive = true` 强制激活。DeskCrate 对应的做法是 `WM_NCACTIVATE` 始终按“激活”交给 `DefWindowProc`。如果实测无效，退回 accent policy 磨砂（Win10 1803+ 已稳定，始终生效）。
- **渲染**：`ID2D1DCRenderTarget`（`D2D1_RENDER_TARGET_TYPE_SOFTWARE`）绑定到一块按需增长、所有窗口共享的 32bpp DIB，`EndDraw` 后 `BitBlt`。不加载 GPU 用户态驱动（这部分常常就占 10+ MB 私有内存）。一个 300×400 的窗口软件光栅只要零点几毫秒，而且只在变化时重绘。
- **文字**：DirectWrite 灰度抗锯齿（透明表面不能用 ClearType），中日韩字体由系统 fallback 处理。可选 1 DIP 文字阴影，在浅色壁纸上保证可读。
- **图标**：`IShellItemImageFactory::GetImage(SIIGBF_ICONONLY)` 按实际像素尺寸取图，转成预乘 BGRA 后缓存为 `ID2D1Bitmap`。普通文件按扩展名共享缓存键，exe、lnk、ico、文件夹按路径缓存。LRU 按字节设上限（默认 4 MB），格子折叠或隐藏时释放。取图放在 STA 工作线程，避免慢速 Shell 扩展卡住 UI。
- **右键菜单**：`IShellFolder::GetUIObjectOf(IContextMenu)` 处理选中项，`CreateViewObject(IContextMenu)` 处理空白处，再把自有菜单项合并进同一个 `HMENU`。转发 `IContextMenu2/3` 的 `WM_INITMENUPOPUP/DRAWITEM/MEASUREITEM/MENUCHAR`，保证“发送到”“打开方式”等子菜单正常。不自绘。
- **拖入**：自己实现的 `IDropTarget` 只负责命中判断和高亮，实际动作 **转交给目标文件夹的 Shell `IDropTarget`**（悬停在子文件夹格上时就转交给该子文件夹）。移动/复制/链接、冲突对话框、进度、撤销、右键拖放菜单都与资源管理器完全一致。拖拽图像由 `IDropTargetHelper` 绘制。
- **拖出**：`GetUIObjectOf(IDataObject)` 后调用 `SHDoDragDrop`，系统自动生成拖拽图像。
- **文件变化**：`ReadDirectoryChangesW` 使用完成例程，在 UI 线程的 `MsgWaitForMultipleObjectsEx(MWMO_ALERTABLE)` 中回调，不需要额外线程，空闲时零唤醒。防抖用一次性 `SetTimer`。

## 5. 架构

### 5.1 线程与进程

- **UI 线程**（STA，`OleInitialize`）：负责所有窗口、托盘、热键、目录监听完成例程、Everything IPC 和配置热重载。
- **图标工作线程**（STA）：在 `Condvar` 上阻塞等待，取图完成后用 `PostMessage` 回 UI 线程；队列为空时 CPU 为 0。
- **单实例**：命名互斥量。第二个实例通过 `WM_COPYDATA` 通知已有实例（呼出抽屉），然后退出。

### 5.2 模块

```
src/
  main.rs          入口：单实例、COM/OLE、消息循环
  app.rs           App 状态与消息分发（模型 ↔ HWND 解耦）
  config.rs        config.toml（toml_edit：读取 + 保留注释写回）+ 默认值
  theme.rs         视觉 token（第 2 节）
  layout.rs        图标格布局公式
  win/             dpi、desktop（贴底/Win+D/Explorer 重启）、material、tray、watcher、hotkey
  shell/           items（枚举/排序）、icons（取图 + LRU）、menu、dragdrop、fileop
  ui/              render（D2D/DWrite 共享渲染器）、grid（格子窗口）、capsule、drawer、textinput
  sort/            规则引擎、计划/Dry-run、执行、撤销日志
  search/          本地过滤、Everything IPC
```

### 5.3 配置（`config.toml`，便携，热重载）

- 默认存放在 exe 同目录。目录不可写时（例如在 Program Files 下）退回 `%LOCALAPPDATA%\DeskCrate\`。
- 用 `toml_edit` 写回，保留用户的注释和排版。拖动、缩放格子只改写对应字段。读取时比对文件哈希，跳过自己写入触发的重载。
- 注册表 **只读**：主题和“隐藏扩展名”设置。唯一的写入是可选的开机自启（`HKCU\...\Run`），关闭时删除。

### 5.4 布局公式（移植自原版 `FileWidgetIconLayout`）

```
labelWidth = max(icon, lerp(icon, text*10.5, nameW))
tileWidth  = max(icon + lerp(6,28,h), labelWidth + lerp(4,16,h))
tileHeight = max(minimum, lerp(1,6,v)*2 + icon + lerp(1,7,v) + ceil(text*1.4)*lines + 2)
margin     = (lerp(0,2,h), lerp(0,2,v))
```

## 6. 性能预算与手段

| 预算 | 手段 | 验证方式 |
| --- | --- | --- |
| exe < 5 MB | `opt-level="s"`、`lto="fat"`、`codegen-units=1`、`panic="abort"`、`strip`；静态 CRT（不依赖 VC++ 运行库）；依赖只有 `windows`、`toml_edit` | `scripts/check-budget.ps1` 读取文件大小 |
| 空闲 < 20 MB | 软件 D2D（不加载 GPU 驱动）；共享 DIB；图标 LRU 上限 4 MB；折叠或隐藏时调用 `EmptyWorkingSet` 并清理缓存 | 脚本读取私有工作集 |
| 空闲 CPU ≈ 0 | 不轮询：悬停用 `TrackMouseEvent`，延迟用一次性定时器，监听用完成例程，全屏检测用 AppBar 的 `ABN_FULLSCREENAPP`；只在动画期间开帧定时器 | 脚本采样 CPU 时间增量 |
| 零残留进程 | 单进程、不派生子进程、固定退出路径 | 退出后检查进程列表 |
| 数据零风险 | 格子 = 真实目录；本程序从不删除文件；归档只移动、不覆盖，并写撤销日志 | 单元测试 + 手工验证 |

## 7. 分阶段计划（每阶段都要能编译、能验证，并且满足预算）

| 阶段 | 内容 | 验收标准 |
| --- | --- | --- |
| **P1 核心底座** | 格子窗口、DWM 材质与圆角、标题栏、图标格、真实目录映射与实时刷新、拖入拖出、原生右键菜单、打开/删除/复制/剪切/粘贴/全选、移动/缩放并持久化、贴底与 `Win+D`、Explorer 重启恢复、PerMonitorV2、托盘（新建格子/打开配置/退出）、配置热重载 | 能编译；exe < 5 MB；空闲 < 20 MB；`Win+D` 后格子仍在；各项交互手工验证通过 |
| **P2 胶囊** | 折叠模式 expanded/click/smart；悬停延迟；220 ms 高度动画；贴边吸附后自动折叠；靠近屏幕底边时向上展开；折叠时释放缓存；内联重命名（F2） | 同上；折叠后内存回落 |
| **P3 双形态** | `mode = drawer`：桌面完全为空；边缘热区（1 px 分层窗口，全屏应用时自动隐藏）和全局热键呼出聚合抽屉，沿用同一套视觉 | 切换不需要重启（热重载） |
| **P4 极简归类** | 监听桌面和下载目录；按扩展名、关键词、闲置天数匹配规则；防抖并跳过 `.crdownload/.part/~$*`；默认 Dry-run 预览；重名自动加后缀、永不覆盖；撤销日志与一键撤销 | 规则引擎单元测试；撤销验证 |
| **P5 检索** | 格子和抽屉内即时过滤（支持 IME）；检测到 Everything 时合并全局结果 | 有无 Everything 两种情况都验证 |
| **P6 收尾** | 预算脚本、README、图标资源、Win10 退化路径、开机自启 | 全部预算达标 |

## 8. 明确不做

天气、音乐、相册、待办、笔记、剪贴板、自定义组件、AI；自绘右键菜单；皮肤和插件商店；更新器、云同步、账号、遥测；对 `explorer.exe` 注入或做破坏性 Hook；缩略图代理进程。

## 9. 实施与验证记录（2026-11-22）

验证环境：Windows 11 22631，单显示器，100% 缩放，系统“透明效果”为关闭状态，Release 构建。

| 阶段 | 结果 | 实测方式 |
| --- | --- | --- |
| P1 核心底座 | ✅ | 截图核对视觉；单实例验证；模拟鼠标完成拖出/拖入（Shell 原生拖拽图像，提示“移动至”）；原生右键菜单截图（含第三方扩展，深色主题）；`ReadDirectoryChangesW` 实时刷新（新建、重命名、删除）；热重载新增格子；模拟 `Win+D` 后格子仍可见，且确认被 DefView 拥有；向宿主窗口发送 `TaskbarCreated` 后格子重新挂载；`WM_CLOSE` 退出后无残留进程 |
| P2 胶囊 | ✅ | smart 模式时序：0.2 s 不展开，0.8 s 已展开；移出 0.4 s 仍展开，1.1 s 已收起。放在屏幕下半区时停靠底边、向上展开。F2 内联重命名（中文、Unicode 输入）走 IFileOperation |
| P3 双形态 | ✅ | 热切到 drawer 后桌面格子全部移除，1 px 边缘热区出现；`Ctrl+Alt+D` 呼出全高抽屉（标签页），再按一次隐藏；边缘悬停呼出，指针移开后自动隐藏；热切回 desktop 后格子恢复 |
| P4 极简归类 | ✅ | 单测：规则匹配、跳过下载中/隐藏/新近修改的文件、重名加后缀不覆盖、执行与撤销往返（真实文件）。实测：事件驱动静止 2 s 后移动 2 个文件，`.crdownload` 被跳过，撤销日志正确写入 |
| P5 检索 | ✅ | 输入即过滤已实测（搜索胶囊、首项选中、Esc 清除）。Everything：单测覆盖 IPC 编码与解析（含越界保护）。本机 Everything 是 OPPO 互联内置的服务版，为不改动第三方软件，用协议一致的模拟 IPC 窗口做了端到端验证：查询包正确，回复在 `SendMessage` 期间重入到达后被正确路由，“全盘结果”分区连同原生图标正常显示 |
| P6 收尾 | ✅ | `scripts/check-budget.ps1 -Launch`：exe **850 KB**（含图标资源），空闲私有内存 **11.2 MB**，空闲 20 s CPU **0.0 ms**，退出干净。clippy 0 警告，15 个单测通过。应用图标由 `build.rs` 程序化绘制并直接写成 `.res`，无需 rc.exe，也不新增依赖 |

### 实施中的发现与偏差

1. **内存根因不在图标**：二分测量显示，Shell 取图几乎不占内存；真正的大头是 D2D/DirectWrite 的进程级字形与排版缓存（中文标签时约 12 MB），`ID2D1Device::ClearResources` 和堆压缩都无法回收。最终方案：渲染器（工厂、RT、文字格式）和各格子缓存的 `IDWriteTextLayout` 在最后一次绘制约 4 s 后整体释放，图标缓存只保留像素，位图按渲染代重建。空闲内存因此从 23.8 MB 降到 10.6 MB，空闲后重绘无慢帧（< 12 ms）。
2. **材质常亮（已修复）**：临时开启“透明效果”后采样发现，从未激活过的窗口从没收到过 `WM_NCACTIVATE`，DWM 一直按未激活画纯色回退（中性灰 31,31,31）。修复办法是应用材质后主动给自己发一次 `WM_NCACTIVATE(TRUE)`，窗口立即呈现带壁纸色调的 Mica（30,31,33）；主题或透明设置变化时也会重发。测试结束后透明设置已恢复为原值 0。说明：任务栏外观由 TranslucentTB 控制，不能作为判断依据。
3. **未覆盖**：多显示器、混合 DPI（逻辑已实现：稳定显示器标识、按 DIP 存储、`WM_DPICHANGED`）、Win10 回退路径、真实的 Explorer 进程重启（只模拟了 `TaskbarCreated`）。
4. **范围收敛**：没有实现原版的列表视图、缩略图和胶囊栏编排；配置中也不保留这些选项的空壳。
